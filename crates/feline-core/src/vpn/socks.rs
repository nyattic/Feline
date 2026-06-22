use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use super::runtime::{EngineCmd, TcpSession};

const SOCKS_VERSION: u8 = 0x05;
const METHOD_USERNAME_PASSWORD: u8 = 0x02;
const METHOD_NO_ACCEPTABLE: u8 = 0xff;
const USERPASS_VERSION: u8 = 0x01;
const USERPASS_STATUS_OK: u8 = 0x00;
const USERPASS_STATUS_DENIED: u8 = 0x01;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;
const REP_OK: u8 = 0x00;
const REP_GENERAL_FAILURE: u8 = 0x01;
const REP_HOST_UNREACHABLE: u8 = 0x04;
const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REP_ADDR_NOT_SUPPORTED: u8 = 0x08;
const SOCKS_USER: &str = "feline";

pub struct SocksHandle {
    pub local_addr: SocketAddr,
    auth_token: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SocksHandle {
    pub fn proxy_url(&self) -> String {
        format!("socks5h://{SOCKS_USER}:{}@{}", self.auth_token, self.local_addr)
    }

    pub fn proxy_display_url(&self) -> String {
        format!("socks5h://{SOCKS_USER}:***@{}", self.local_addr)
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for SocksHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn start(cmd_tx: mpsc::Sender<EngineCmd>) -> Result<SocksHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind SOCKS5 listener")?;
    let local_addr = listener.local_addr().context("read SOCKS5 local_addr")?;
    let auth_token = random_auth_token();
    let auth_token_for_task = auth_token.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    tracing::info!("socks5 listener shutting down");
                    break;
                }
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, peer)) => {
                            tracing::debug!(?peer, "socks5 accepted");
                            let cmd_tx = cmd_tx.clone();
                            let auth_token = auth_token_for_task.clone();
                            tokio::spawn(async move {
                                if let Err(err) = handle_client(stream, cmd_tx, &auth_token).await {
                                    tracing::warn!(error = %format!("{err:#}"), "socks5 client error");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(%err, "socks5 accept error");
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    }
                }
            }
        }
    });

    Ok(SocksHandle {
        local_addr,
        auth_token,
        shutdown: Some(shutdown_tx),
        task: Some(task),
    })
}

fn random_auth_token() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn handle_client(
    mut stream: TcpStream,
    cmd_tx: mpsc::Sender<EngineCmd>,
    auth_token: &str,
) -> Result<()> {
    negotiate_method(&mut stream, auth_token).await?;
    let target = read_request(&mut stream).await?;

    let dst = match resolve(&target, &cmd_tx).await {
        Ok(addr) => addr,
        Err(err) => {
            write_reply(&mut stream, REP_HOST_UNREACHABLE).await?;
            return Err(err);
        }
    };

    let (tx, rx) = oneshot::channel();
    cmd_tx
        .send(EngineCmd::OpenTcp { dst, reply: tx })
        .await
        .map_err(|_| anyhow!("vpn engine closed before OpenTcp"))?;
    let mut session = match rx.await {
        Ok(Ok(s)) => s,
        Ok(Err(err)) => {
            write_reply(&mut stream, REP_GENERAL_FAILURE).await?;
            return Err(err);
        }
        Err(_) => {
            write_reply(&mut stream, REP_GENERAL_FAILURE).await?;
            bail!("vpn engine dropped OpenTcp reply");
        }
    };

    match session.connected.take() {
        Some(connected) => match connected.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                write_reply(&mut stream, REP_HOST_UNREACHABLE).await?;
                return Err(err);
            }
            Err(_) => {
                write_reply(&mut stream, REP_GENERAL_FAILURE).await?;
                bail!("vpn engine dropped connect notification");
            }
        },
        None => {
            write_reply(&mut stream, REP_GENERAL_FAILURE).await?;
            bail!("vpn engine omitted connect notification");
        }
    }

    write_reply(&mut stream, REP_OK).await?;
    bridge(stream, session).await
}

async fn negotiate_method(stream: &mut TcpStream, auth_token: &str) -> Result<()> {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .await
        .context("read SOCKS5 greeting")?;
    if header[0] != SOCKS_VERSION {
        bail!("unsupported SOCKS version {:#x}", header[0]);
    }
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    stream
        .read_exact(&mut methods)
        .await
        .context("read SOCKS5 methods")?;

    let choice = if methods.contains(&METHOD_USERNAME_PASSWORD) {
        METHOD_USERNAME_PASSWORD
    } else {
        stream
            .write_all(&[SOCKS_VERSION, METHOD_NO_ACCEPTABLE])
            .await
            .ok();
        bail!("client offered no acceptable SOCKS5 auth method");
    };
    stream
        .write_all(&[SOCKS_VERSION, choice])
        .await
        .context("write SOCKS5 method ack")?;
    authenticate_userpass(stream, auth_token).await?;
    Ok(())
}

async fn authenticate_userpass(stream: &mut TcpStream, auth_token: &str) -> Result<()> {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .await
        .context("read SOCKS5 username/password header")?;
    if header[0] != USERPASS_VERSION {
        write_userpass_status(stream, USERPASS_STATUS_DENIED).await?;
        bail!(
            "unsupported SOCKS5 username/password version {:#x}",
            header[0]
        );
    }

    let username_len = header[1] as usize;
    let mut username = vec![0u8; username_len];
    stream
        .read_exact(&mut username)
        .await
        .context("read SOCKS5 username")?;

    let mut pass_len = [0u8; 1];
    stream
        .read_exact(&mut pass_len)
        .await
        .context("read SOCKS5 password length")?;
    let mut password = vec![0u8; pass_len[0] as usize];
    stream
        .read_exact(&mut password)
        .await
        .context("read SOCKS5 password")?;

    if username == SOCKS_USER.as_bytes() && password == auth_token.as_bytes() {
        write_userpass_status(stream, USERPASS_STATUS_OK).await?;
        Ok(())
    } else {
        write_userpass_status(stream, USERPASS_STATUS_DENIED).await?;
        bail!("SOCKS5 authentication failed");
    }
}

async fn write_userpass_status(stream: &mut TcpStream, status: u8) -> Result<()> {
    stream
        .write_all(&[USERPASS_VERSION, status])
        .await
        .context("write SOCKS5 username/password status")
}

enum Target {
    Addr(SocketAddr),
    Domain(String, u16),
}

async fn read_request(stream: &mut TcpStream) -> Result<Target> {
    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .context("read SOCKS5 request header")?;
    if head[0] != SOCKS_VERSION {
        bail!("unsupported SOCKS version in request {:#x}", head[0]);
    }
    if head[1] != CMD_CONNECT {
        write_reply(stream, REP_COMMAND_NOT_SUPPORTED).await?;
        bail!("only CONNECT is supported (got cmd {:#x})", head[1]);
    }

    let target = match head[3] {
        ATYP_IPV4 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            let port = read_port(stream).await?;
            Target::Addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port))
        }
        ATYP_IPV6 => {
            let mut octets = [0u8; 16];
            stream.read_exact(&mut octets).await?;
            let port = read_port(stream).await?;
            Target::Addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            stream.read_exact(&mut name).await?;
            let port = read_port(stream).await?;
            let host = String::from_utf8(name).context("SOCKS5 domain not UTF-8")?;
            Target::Domain(host, port)
        }
        other => {
            write_reply(stream, REP_ADDR_NOT_SUPPORTED).await?;
            bail!("unsupported address type {:#x}", other);
        }
    };
    Ok(target)
}

async fn read_port(stream: &mut TcpStream) -> Result<u16> {
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await?;
    Ok(u16::from_be_bytes(buf))
}

async fn resolve(target: &Target, cmd_tx: &mpsc::Sender<EngineCmd>) -> Result<SocketAddr> {
    match target {
        Target::Addr(a) => Ok(*a),
        Target::Domain(host, port) => {
            let (tx, rx) = oneshot::channel();
            cmd_tx
                .send(EngineCmd::ResolveHost {
                    domain: host.clone(),
                    reply: tx,
                })
                .await
                .map_err(|_| anyhow!("vpn engine closed before DNS query"))?;
            let ips = rx
                .await
                .map_err(|_| anyhow!("vpn engine dropped DNS reply"))?
                .with_context(|| format!("resolve `{host}` through VPN tunnel"))?;
            let ip = ips
                .into_iter()
                .find(|i| i.is_ipv4())
                .ok_or_else(|| anyhow!("`{host}` resolved to no IPv4 addresses"))?;
            Ok(SocketAddr::new(ip, *port))
        }
    }
}

async fn write_reply(stream: &mut TcpStream, rep: u8) -> Result<()> {
    let response = [SOCKS_VERSION, rep, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0];
    stream
        .write_all(&response)
        .await
        .context("write SOCKS5 reply")
}

async fn bridge(stream: TcpStream, mut session: TcpSession) -> Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();
    let cmd_tx = session.cmd_tx.clone();
    let handle = session.handle;

    let upload = tokio::spawn(async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match read_half.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if cmd_tx
                        .send(EngineCmd::OutboundData {
                            handle,
                            data: buf[..n].to_vec(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = cmd_tx.send(EngineCmd::CloseTcp { handle }).await;
    });

    let download = tokio::spawn(async move {
        while let Some(data) = session.inbound.recv().await {
            if write_half.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = write_half.shutdown().await;
    });

    let _ = tokio::join!(upload, download);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_token_is_url_safe() {
        let token = random_auth_token();
        assert_eq!(token.len(), 32);
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn display_url_redacts_auth_token() {
        let handle = SocksHandle {
            local_addr: "127.0.0.1:12345".parse().unwrap(),
            auth_token: "secret-token".to_string(),
            shutdown: None,
            task: None,
        };
        assert_eq!(
            handle.proxy_url(),
            "socks5h://feline:secret-token@127.0.0.1:12345"
        );
        assert_eq!(
            handle.proxy_display_url(),
            "socks5h://feline:***@127.0.0.1:12345"
        );
    }
}
