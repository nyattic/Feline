use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant as StdInstant};

use anyhow::{Context, Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr as SmolCidr, IpEndpoint};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

use super::config::{IpCidr, WgConfig};
use super::device::{PacketQueues, VirtualDevice};
use super::dns;

const MAX_PACKET: usize = 65535;
const TCP_RX_BUF: usize = 256 * 1024;
const TCP_TX_BUF: usize = 256 * 1024;
const DEFAULT_MTU: usize = 1420;
const CHANNEL_DEPTH: usize = 256;
const EPHEMERAL_PORT_BASE: u16 = 49152;
const EPHEMERAL_PORT_END: u16 = 65535;
const UPDATE_TIMER_INTERVAL: Duration = Duration::from_millis(250);
const ACTIVE_POLL_MAX: Duration = Duration::from_millis(50);
const IDLE_POLL_MAX: Duration = Duration::from_millis(250);
const DNS_LOCAL_PORT: u16 = 35353;
const DNS_REMOTE_PORT: u16 = 53;
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const DNS_PKT_BUF: usize = 4096;
const DNS_META_SLOTS: usize = 16;
const FALLBACK_DNS: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);

pub enum EngineCmd {
    OpenTcp {
        dst: SocketAddr,
        reply: oneshot::Sender<Result<TcpSession>>,
    },
    OutboundData {
        handle: SocketHandle,
        data: Vec<u8>,
    },
    CloseTcp {
        handle: SocketHandle,
    },
    ResolveHost {
        domain: String,
        reply: oneshot::Sender<Result<Vec<IpAddr>>>,
    },
}

pub struct TcpSession {
    pub handle: SocketHandle,
    pub cmd_tx: mpsc::Sender<EngineCmd>,
    pub inbound: mpsc::Receiver<Vec<u8>>,
}

pub struct EngineHandle {
    cmd_tx: mpsc::Sender<EngineCmd>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl EngineHandle {
    pub fn cmd_sender(&self) -> mpsc::Sender<EngineCmd> {
        self.cmd_tx.clone()
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

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn start(cfg: WgConfig) -> Result<EngineHandle> {
    let prepared = PreparedConfig::from_wg(&cfg)?;
    let (cmd_tx, cmd_rx) = mpsc::channel(CHANNEL_DEPTH);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let cmd_tx_clone = cmd_tx.clone();

    let task = tokio::spawn(async move {
        if let Err(err) = run_engine(prepared, cmd_rx, shutdown_rx, cmd_tx_clone).await {
            tracing::error!(error = %format!("{err:#}"), "vpn engine exited with error");
        }
    });

    Ok(EngineHandle {
        cmd_tx,
        shutdown: Some(shutdown_tx),
        task: Some(task),
    })
}

struct PreparedConfig {
    static_secret: StaticSecret,
    peer_public: PublicKey,
    preshared_key: Option<[u8; 32]>,
    persistent_keepalive: Option<u16>,
    endpoint: SocketAddr,
    source_addrs: Vec<IpCidr>,
    dns_servers: Vec<IpAddr>,
    mtu: usize,
}

impl PreparedConfig {
    fn from_wg(cfg: &WgConfig) -> Result<Self> {
        let static_secret =
            StaticSecret::from(decode_key(&cfg.interface.private_key, "PrivateKey")?);
        let peer_public = PublicKey::from(decode_key(&cfg.peer.public_key, "PublicKey")?);
        let preshared_key = match cfg.peer.preshared_key.as_deref() {
            Some(s) => Some(decode_key(s, "PresharedKey")?),
            None => None,
        };
        let endpoint = resolve_endpoint(&cfg.peer.endpoint)?;
        let mut dns_servers: Vec<IpAddr> = cfg
            .interface
            .dns
            .iter()
            .filter(|a| a.is_ipv4())
            .copied()
            .collect();
        if dns_servers.is_empty() {
            dns_servers.push(IpAddr::V4(FALLBACK_DNS));
        }
        Ok(Self {
            static_secret,
            peer_public,
            preshared_key,
            persistent_keepalive: cfg.peer.persistent_keepalive,
            endpoint,
            source_addrs: cfg.interface.addresses.clone(),
            dns_servers,
            mtu: cfg.interface.mtu.map(|m| m as usize).unwrap_or(DEFAULT_MTU),
        })
    }

    fn source_ipv4(&self) -> Result<Ipv4Addr> {
        for cidr in &self.source_addrs {
            if let IpAddr::V4(v4) = cidr.addr {
                return Ok(v4);
            }
        }
        Err(anyhow!("VPN config has no IPv4 Address for interface"))
    }
}

fn decode_key(input: &str, field: &str) -> Result<[u8; 32]> {
    let bytes = BASE64
        .decode(input.trim().as_bytes())
        .with_context(|| format!("{field} is not valid base64"))?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("{field} decodes to {} bytes, expected 32", v.len()))
}

fn resolve_endpoint(s: &str) -> Result<SocketAddr> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let mut iter = std::net::ToSocketAddrs::to_socket_addrs(s)
        .with_context(|| format!("failed to resolve endpoint `{s}`"))?;
    iter.next()
        .ok_or_else(|| anyhow!("endpoint `{s}` resolved to no addresses"))
}

struct ManagedSocket {
    inbound_tx: mpsc::Sender<Vec<u8>>,
    pending_outbound: std::collections::VecDeque<Vec<u8>>,
    half_close_requested: bool,
}

type DnsReplySender = oneshot::Sender<Result<Vec<IpAddr>>>;
type PendingDns = HashMap<u16, (DnsReplySender, StdInstant)>;

async fn run_engine(
    cfg: PreparedConfig,
    mut cmd_rx: mpsc::Receiver<EngineCmd>,
    mut shutdown_rx: oneshot::Receiver<()>,
    cmd_tx_for_sessions: mpsc::Sender<EngineCmd>,
) -> Result<()> {
    let udp = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("bind UDP socket for VPN")?;
    udp.connect(cfg.endpoint)
        .await
        .with_context(|| format!("connect UDP to VPN endpoint {}", cfg.endpoint))?;

    let mut tunn = Tunn::new(
        cfg.static_secret.clone(),
        cfg.peer_public,
        cfg.preshared_key,
        cfg.persistent_keepalive,
        0,
        None,
    );

    let queues = PacketQueues::new();
    let mut device = VirtualDevice::new(queues.clone(), cfg.mtu);

    let iface_config = IfaceConfig::new(HardwareAddress::Ip);
    let mut iface = Interface::new(iface_config, &mut device, SmolInstant::now());

    let source_ipv4 = cfg.source_ipv4()?;
    iface.update_ip_addrs(|ips| {
        for cidr in &cfg.source_addrs {
            let prefix = cidr.prefix;
            let addr = IpAddress::from(cidr.addr);
            if let Err(err) = ips.push(SmolCidr::new(addr, prefix)) {
                tracing::warn!(?err, "could not add VPN interface address");
            }
        }
    });

    let mut sockets = SocketSet::new(Vec::new());
    let mut managed: HashMap<SocketHandle, ManagedSocket> = HashMap::new();
    let mut next_port: u16 = EPHEMERAL_PORT_BASE;
    let mut update_timer = tokio::time::interval(UPDATE_TIMER_INTERVAL);
    update_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut udp_buf = vec![0u8; MAX_PACKET];
    let mut send_scratch = vec![0u8; MAX_PACKET];

    let dns_rx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; DNS_META_SLOTS],
        vec![0u8; DNS_PKT_BUF],
    );
    let dns_tx_buf = udp::PacketBuffer::new(
        vec![udp::PacketMetadata::EMPTY; DNS_META_SLOTS],
        vec![0u8; DNS_PKT_BUF],
    );
    let mut dns_socket = udp::Socket::new(dns_rx_buf, dns_tx_buf);
    dns_socket
        .bind(DNS_LOCAL_PORT)
        .map_err(|err| anyhow!("bind DNS UDP socket on {DNS_LOCAL_PORT}: {err:?}"))?;
    let dns_handle = sockets.add(dns_socket);
    let mut pending_dns: PendingDns = HashMap::new();
    let mut dns_id_counter: u16 = 1;
    let dns_server = cfg.dns_servers[0];

    flush_tunn_handshake(&mut tunn, &udp, &mut send_scratch).await;

    loop {
        let poll_delay = iface.poll_delay(SmolInstant::now(), &sockets);
        let cap_ms = if managed.is_empty() {
            IDLE_POLL_MAX.as_millis() as u64
        } else {
            ACTIVE_POLL_MAX.as_millis() as u64
        };
        let next_wake = match poll_delay {
            Some(smoltcp::time::Duration::ZERO) => Duration::from_millis(0),
            Some(d) => Duration::from_millis(d.total_millis().min(cap_ms)),
            None => Duration::from_millis(cap_ms),
        };

        tokio::select! {
            biased;

            _ = &mut shutdown_rx => {
                tracing::info!("vpn engine: shutdown signal received");
                break;
            }

            r = udp.recv(&mut udp_buf) => {
                match r {
                    Ok(n) => {
                        let data = udp_buf[..n].to_vec();
                        handle_udp_in(&mut tunn, &udp, &queues, &data, &mut send_scratch).await;
                    }
                    Err(err) => {
                        tracing::warn!(%err, "vpn UDP recv error");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(EngineCmd::OpenTcp { dst, reply }) => {
                        let result = open_tcp_session(
                            &mut sockets,
                            &mut iface,
                            &mut managed,
                            &mut next_port,
                            source_ipv4,
                            dst,
                            &cmd_tx_for_sessions,
                        );
                        let _ = reply.send(result);
                    }
                    Some(EngineCmd::OutboundData { handle, data }) => {
                        if let Some(mgr) = managed.get_mut(&handle) {
                            mgr.pending_outbound.push_back(data);
                        }
                    }
                    Some(EngineCmd::CloseTcp { handle }) => {
                        if let Some(mgr) = managed.get_mut(&handle) {
                            mgr.half_close_requested = true;
                        }
                    }
                    Some(EngineCmd::ResolveHost { domain, reply }) => {
                        send_dns_query(
                            &mut sockets,
                            dns_handle,
                            dns_server,
                            &mut dns_id_counter,
                            &mut pending_dns,
                            domain,
                            reply,
                        );
                    }
                    None => {
                        tracing::info!("vpn engine: command channel closed");
                        break;
                    }
                }
            }

            _ = update_timer.tick() => {
                drive_timers(&mut tunn, &udp, &mut send_scratch).await;
            }

            _ = tokio::time::sleep(next_wake) => {}
        }

        let now = SmolInstant::now();
        iface.poll(now, &mut device, &mut sockets);

        pump_sessions(&mut sockets, &mut managed);
        drain_dns_responses(&mut sockets, dns_handle, &mut pending_dns);
        flush_outbound(&queues, &mut tunn, &udp, &mut send_scratch).await;

        sweep_closed(&mut sockets, &mut managed);
        expire_dns(&mut pending_dns);
    }

    Ok(())
}

fn open_tcp_session(
    sockets: &mut SocketSet<'static>,
    iface: &mut Interface,
    managed: &mut HashMap<SocketHandle, ManagedSocket>,
    next_port: &mut u16,
    source_ipv4: Ipv4Addr,
    dst: SocketAddr,
    cmd_tx: &mpsc::Sender<EngineCmd>,
) -> Result<TcpSession> {
    let rx_buf = tcp::SocketBuffer::new(vec![0u8; TCP_RX_BUF]);
    let tx_buf = tcp::SocketBuffer::new(vec![0u8; TCP_TX_BUF]);
    let mut socket = tcp::Socket::new(rx_buf, tx_buf);
    socket.set_nagle_enabled(false);
    socket.set_keep_alive(Some(smoltcp::time::Duration::from_secs(30)));

    let local_port = allocate_port(next_port);
    let local_endpoint = (IpAddress::from(IpAddr::V4(source_ipv4)), local_port);
    let remote_endpoint = (IpAddress::from(dst.ip()), dst.port());

    let cx = iface.context();
    socket
        .connect(cx, remote_endpoint, local_endpoint)
        .with_context(|| format!("smoltcp connect to {dst}"))?;

    let handle = sockets.add(socket);
    let (inbound_tx, inbound_rx) = mpsc::channel(CHANNEL_DEPTH);
    managed.insert(
        handle,
        ManagedSocket {
            inbound_tx,
            pending_outbound: Default::default(),
            half_close_requested: false,
        },
    );

    Ok(TcpSession {
        handle,
        cmd_tx: cmd_tx.clone(),
        inbound: inbound_rx,
    })
}

fn allocate_port(next_port: &mut u16) -> u16 {
    let port = *next_port;
    *next_port = if *next_port == EPHEMERAL_PORT_END {
        EPHEMERAL_PORT_BASE
    } else {
        *next_port + 1
    };
    port
}

fn pump_sessions(
    sockets: &mut SocketSet<'static>,
    managed: &mut HashMap<SocketHandle, ManagedSocket>,
) {
    for (handle, mgr) in managed.iter_mut() {
        let sock = sockets.get_mut::<tcp::Socket>(*handle);

        while sock.can_send() {
            let Some(front) = mgr.pending_outbound.front_mut() else {
                break;
            };
            match sock.send_slice(front) {
                Ok(sent) if sent == front.len() => {
                    mgr.pending_outbound.pop_front();
                }
                Ok(sent) => {
                    front.drain(..sent);
                    break;
                }
                Err(_) => break,
            }
        }

        if mgr.half_close_requested && mgr.pending_outbound.is_empty() && sock.may_send() {
            sock.close();
        }

        while sock.can_recv() {
            if mgr.inbound_tx.capacity() == 0 {
                break;
            }
            let drained = sock.recv(|buf| {
                let len = buf.len();
                (len, buf.to_vec())
            });
            match drained {
                Ok(data) if !data.is_empty() => {
                    use tokio::sync::mpsc::error::TrySendError;
                    match mgr.inbound_tx.try_send(data) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => break,
                        Err(TrySendError::Closed(_)) => {
                            sock.close();
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }
}

fn sweep_closed(
    sockets: &mut SocketSet<'static>,
    managed: &mut HashMap<SocketHandle, ManagedSocket>,
) {
    let mut to_remove = Vec::new();
    for (handle, _) in managed.iter() {
        let sock = sockets.get::<tcp::Socket>(*handle);
        if matches!(sock.state(), tcp::State::Closed) {
            to_remove.push(*handle);
        }
    }
    for handle in to_remove {
        managed.remove(&handle);
        sockets.remove(handle);
    }
}

async fn handle_udp_in(
    tunn: &mut Tunn,
    udp: &UdpSocket,
    queues: &PacketQueues,
    data: &[u8],
    scratch: &mut [u8],
) {
    match tunn.decapsulate(None, data, scratch) {
        TunnResult::WriteToNetwork(packet) => {
            let _ = udp.send(packet).await;
            loop {
                let mut more = vec![0u8; MAX_PACKET];
                match tunn.decapsulate(None, &[], &mut more) {
                    TunnResult::WriteToNetwork(p) => {
                        let _ = udp.send(p).await;
                    }
                    _ => break,
                }
            }
        }
        TunnResult::WriteToTunnelV4(packet, _) | TunnResult::WriteToTunnelV6(packet, _) => {
            queues.push_inbound(packet.to_vec());
        }
        TunnResult::Done => {}
        TunnResult::Err(WireGuardError::DuplicateCounter) => {
            tracing::trace!("vpn decapsulate: dropped duplicate packet");
        }
        TunnResult::Err(err) => {
            tracing::warn!(?err, "vpn decapsulate error");
        }
    }
}

async fn flush_outbound(
    queues: &PacketQueues,
    tunn: &mut Tunn,
    udp: &UdpSocket,
    scratch: &mut [u8],
) {
    for packet in queues.drain_outbound() {
        match tunn.encapsulate(&packet, scratch) {
            TunnResult::WriteToNetwork(out) => {
                let _ = udp.send(out).await;
            }
            TunnResult::Done => {}
            TunnResult::Err(err) => {
                tracing::warn!(?err, "vpn encapsulate error");
            }
            other => {
                tracing::warn!(?other, "unexpected encapsulate result");
            }
        }
    }
}

async fn drive_timers(tunn: &mut Tunn, udp: &UdpSocket, scratch: &mut [u8]) {
    loop {
        match tunn.update_timers(scratch) {
            TunnResult::WriteToNetwork(packet) => {
                let _ = udp.send(packet).await;
            }
            TunnResult::Err(WireGuardError::ConnectionExpired) => {
                let mut init = vec![0u8; MAX_PACKET];
                if let TunnResult::WriteToNetwork(p) =
                    tunn.format_handshake_initiation(&mut init, false)
                {
                    let _ = udp.send(p).await;
                }
                break;
            }
            TunnResult::Err(err) => {
                tracing::warn!(?err, "vpn update_timers error");
                break;
            }
            TunnResult::Done => break,
            other => {
                tracing::warn!(?other, "unexpected update_timers result");
                break;
            }
        }
    }
}

async fn flush_tunn_handshake(tunn: &mut Tunn, udp: &UdpSocket, scratch: &mut [u8]) {
    if let TunnResult::WriteToNetwork(p) = tunn.format_handshake_initiation(scratch, false) {
        let _ = udp.send(p).await;
    }
}

fn send_dns_query(
    sockets: &mut SocketSet<'static>,
    dns_handle: SocketHandle,
    dns_server: IpAddr,
    dns_id_counter: &mut u16,
    pending: &mut PendingDns,
    domain: String,
    reply: DnsReplySender,
) {
    let id = next_dns_id(dns_id_counter);
    let query = match dns::build_query(id, &domain, dns::TYPE_A) {
        Ok(q) => q,
        Err(err) => {
            let _ = reply.send(Err(err));
            return;
        }
    };
    let endpoint = IpEndpoint::new(IpAddress::from(dns_server), DNS_REMOTE_PORT);
    let sock = sockets.get_mut::<udp::Socket>(dns_handle);
    match sock.send_slice(&query, endpoint) {
        Ok(()) => {
            pending.insert(id, (reply, StdInstant::now() + DNS_TIMEOUT));
        }
        Err(err) => {
            let _ = reply.send(Err(anyhow!("DNS send failed: {err:?}")));
        }
    }
}

fn next_dns_id(counter: &mut u16) -> u16 {
    let id = *counter;
    *counter = match counter.checked_add(1) {
        Some(v) if v != 0 => v,
        _ => 1,
    };
    id
}

fn drain_dns_responses(
    sockets: &mut SocketSet<'static>,
    dns_handle: SocketHandle,
    pending: &mut PendingDns,
) {
    loop {
        let sock = sockets.get_mut::<udp::Socket>(dns_handle);
        let payload = match sock.recv() {
            Ok((data, _meta)) => data.to_vec(),
            Err(_) => break,
        };
        if payload.len() < 12 {
            continue;
        }
        let id = u16::from_be_bytes([payload[0], payload[1]]);
        let Some((reply, _)) = pending.remove(&id) else {
            continue;
        };
        let _ = reply.send(dns::parse_response(&payload, id));
    }
}

fn expire_dns(pending: &mut PendingDns) {
    let now = StdInstant::now();
    let expired: Vec<u16> = pending
        .iter()
        .filter_map(|(id, (_, deadline))| (now > *deadline).then_some(*id))
        .collect();
    for id in expired {
        if let Some((reply, _)) = pending.remove(&id) {
            let _ = reply.send(Err(anyhow!("DNS query {id:#06x} timed out")));
        }
    }
}
