use anyhow::{Context, Result, anyhow};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use rustls::client::{EchConfig, EchMode};
use rustls::crypto::aws_lc_rs::{default_provider, hpke::ALL_SUPPORTED_SUITES};
use rustls::pki_types::EchConfigListBytes;
use rustls_platform_verifier::BuilderVerifierExt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

const DOH_URL: &str = "https://cloudflare-dns.com/dns-query";
const DOH_HOST: &str = "cloudflare-dns.com";
const HTTPS_RR: u16 = 65;
const A_RR: u16 = 1;
const AAAA_RR: u16 = 28;
const CLASS_IN: u16 = 1;
const SVC_PARAM_IPV4HINT: u16 = 4;
const SVC_PARAM_ECH: u16 = 5;
const SVC_PARAM_IPV6HINT: u16 = 6;

#[derive(Debug, Clone, Default)]
pub struct EchDnsConfig {
    pub ech_config_list: Vec<u8>,
    pub addrs: Vec<SocketAddr>,
}

pub async fn configure_ech_client(
    builder: reqwest::ClientBuilder,
    host: &str,
    fail_closed: bool,
) -> anyhow::Result<reqwest::ClientBuilder> {
    match fetch_ech_dns_config(host).await {
        Ok(cfg) => match build_ech_tls_config(&cfg) {
            Ok(tls) => {
                tracing::info!(
                    host,
                    addrs = cfg.addrs.len(),
                    "enabled ECH for API connections"
                );
                let b = builder.use_preconfigured_tls(tls);
                Ok(if cfg.addrs.is_empty() {
                    b
                } else {
                    b.resolve_to_addrs(host, &cfg.addrs)
                })
            }
            Err(e) if fail_closed => Err(e.context("ECH TLS config required but unavailable")),
            Err(e) => {
                tracing::warn!(host, "ECH TLS config unavailable, using normal TLS: {e:#}");
                Ok(builder)
            }
        },
        Err(e) if fail_closed => Err(e.context("ECH DNS config required but unavailable")),
        Err(e) => {
            tracing::warn!(host, "ECH DNS config unavailable, using normal TLS: {e:#}");
            Ok(builder)
        }
    }
}

async fn fetch_ech_dns_config(host: &str) -> Result<EchDnsConfig> {
    let doh = doh_client()?;
    let message = build_query(host, HTTPS_RR)?;
    let response = doh
        .post(DOH_URL)
        .header(CONTENT_TYPE, "application/dns-message")
        .header(ACCEPT, "application/dns-message")
        .body(message)
        .send()
        .await
        .context("send DoH HTTPS query")?
        .error_for_status()
        .context("DoH HTTPS query status")?
        .bytes()
        .await
        .context("read DoH HTTPS response")?;

    parse_https_response(&response).context("parse DoH HTTPS response")
}

fn doh_client() -> Result<reqwest::Client> {
    let addrs = [
        SocketAddr::from(([1, 1, 1, 1], 443)),
        SocketAddr::from(([1, 0, 0, 1], 443)),
        SocketAddr::from(([2606, 4700, 4700, 0, 0, 0, 0, 1111], 443)),
        SocketAddr::from(([2606, 4700, 4700, 0, 0, 0, 0, 1001], 443)),
    ];

    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .resolve_to_addrs(DOH_HOST, &addrs)
        .build()
        .context("build DoH client")
}

fn build_ech_tls_config(cfg: &EchDnsConfig) -> Result<rustls::ClientConfig> {
    let ech = EchConfig::new(
        EchConfigListBytes::from(cfg.ech_config_list.clone()),
        ALL_SUPPORTED_SUITES,
    )
    .context("select compatible ECH config")?;

    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_ech(EchMode::Enable(ech))
        .context("enable ECH")?
        .with_platform_verifier()
        .context("configure platform verifier")?
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(tls)
}

fn build_query(host: &str, qtype: u16) -> Result<Vec<u8>> {
    if host.is_empty() {
        return Err(anyhow!("empty DNS host"));
    }

    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&0x4645_u16.to_be_bytes());
    out.extend_from_slice(&0x0100_u16.to_be_bytes());
    out.extend_from_slice(&1_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    encode_name(&mut out, host)?;
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(out)
}

fn encode_name(out: &mut Vec<u8>, name: &str) -> Result<()> {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(anyhow!("invalid DNS label in {name}"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

fn parse_https_response(bytes: &[u8]) -> Result<EchDnsConfig> {
    if bytes.len() < 12 {
        return Err(anyhow!("DNS response is too short"));
    }
    let qd = read_u16(bytes, 4)? as usize;
    let an = read_u16(bytes, 6)? as usize;
    let ns = read_u16(bytes, 8)? as usize;
    let ar = read_u16(bytes, 10)? as usize;

    let mut pos = 12;
    for _ in 0..qd {
        skip_name(bytes, &mut pos)?;
        pos = pos
            .checked_add(4)
            .ok_or_else(|| anyhow!("DNS question overflow"))?;
        ensure_len(bytes, pos)?;
    }

    let mut cfg = EchDnsConfig::default();
    for _ in 0..(an + ns + ar) {
        parse_rr(bytes, &mut pos, &mut cfg)?;
    }

    if cfg.ech_config_list.is_empty() {
        return Err(anyhow!("HTTPS record did not include an ECH parameter"));
    }
    Ok(cfg)
}

fn parse_rr(bytes: &[u8], pos: &mut usize, cfg: &mut EchDnsConfig) -> Result<()> {
    skip_name(bytes, pos)?;
    let rr_type = read_u16_at(bytes, pos)?;
    let rr_class = read_u16_at(bytes, pos)?;
    let _ttl = read_u32_at(bytes, pos)?;
    let rdlen = read_u16_at(bytes, pos)? as usize;
    let end = pos
        .checked_add(rdlen)
        .ok_or_else(|| anyhow!("DNS rdata overflow"))?;
    ensure_len(bytes, end)?;

    if rr_class == CLASS_IN {
        match rr_type {
            HTTPS_RR => parse_https_rdata(bytes, *pos, end, cfg)?,
            A_RR if rdlen == 4 => cfg.addrs.push(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(
                    bytes[*pos],
                    bytes[*pos + 1],
                    bytes[*pos + 2],
                    bytes[*pos + 3],
                )),
                443,
            )),
            AAAA_RR if rdlen == 16 => {
                let octets: [u8; 16] = bytes[*pos..end].try_into().expect("length checked");
                cfg.addrs
                    .push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), 443));
            }
            _ => {}
        }
    }

    *pos = end;
    Ok(())
}

fn parse_https_rdata(
    bytes: &[u8],
    mut pos: usize,
    end: usize,
    cfg: &mut EchDnsConfig,
) -> Result<()> {
    let _priority = read_u16_at(bytes, &mut pos)?;
    skip_name_until(bytes, &mut pos, end)?;

    while pos < end {
        let key = read_u16_at(bytes, &mut pos)?;
        let len = read_u16_at(bytes, &mut pos)? as usize;
        let value_end = pos
            .checked_add(len)
            .ok_or_else(|| anyhow!("SvcParam overflow"))?;
        if value_end > end {
            return Err(anyhow!("SvcParam exceeds HTTPS rdata"));
        }
        let value = &bytes[pos..value_end];
        match key {
            SVC_PARAM_ECH => cfg.ech_config_list = value.to_vec(),
            SVC_PARAM_IPV4HINT => {
                for chunk in value.chunks_exact(4) {
                    cfg.addrs.push(SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3])),
                        443,
                    ));
                }
            }
            SVC_PARAM_IPV6HINT => {
                for chunk in value.chunks_exact(16) {
                    let octets: [u8; 16] = chunk.try_into().expect("chunk length checked");
                    cfg.addrs
                        .push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), 443));
                }
            }
            _ => {}
        }
        pos = value_end;
    }

    Ok(())
}

fn skip_name(bytes: &[u8], pos: &mut usize) -> Result<()> {
    skip_name_until(bytes, pos, bytes.len())
}

fn skip_name_until(bytes: &[u8], pos: &mut usize, end: usize) -> Result<()> {
    let mut jumped = false;
    let mut cursor = *pos;
    let mut hops = 0;
    loop {
        if cursor >= end || cursor >= bytes.len() {
            return Err(anyhow!("DNS name exceeds message"));
        }
        let len = bytes[cursor];
        if len & 0xc0 == 0xc0 {
            if cursor + 1 >= bytes.len() {
                return Err(anyhow!("truncated DNS compression pointer"));
            }
            let ptr = (((len & 0x3f) as usize) << 8) | bytes[cursor + 1] as usize;
            if !jumped {
                *pos = cursor + 2;
            }
            cursor = ptr;
            jumped = true;
            hops += 1;
            if hops > 16 {
                return Err(anyhow!("too many DNS compression pointers"));
            }
            continue;
        }
        if len == 0 {
            if !jumped {
                *pos = cursor + 1;
            }
            return Ok(());
        }
        if len & 0xc0 != 0 {
            return Err(anyhow!("unsupported DNS label type"));
        }
        cursor += 1 + len as usize;
    }
}

fn read_u16(bytes: &[u8], pos: usize) -> Result<u16> {
    ensure_len(bytes, pos + 2)?;
    Ok(u16::from_be_bytes([bytes[pos], bytes[pos + 1]]))
}

fn read_u16_at(bytes: &[u8], pos: &mut usize) -> Result<u16> {
    let value = read_u16(bytes, *pos)?;
    *pos += 2;
    Ok(value)
}

fn read_u32_at(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    ensure_len(bytes, *pos + 4)?;
    let value = u32::from_be_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
    ]);
    *pos += 4;
    Ok(value)
}

fn ensure_len(bytes: &[u8], len: usize) -> Result<()> {
    if bytes.len() < len {
        Err(anyhow!("truncated DNS message"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_type65_query() {
        let query = build_query("e621.net", HTTPS_RR).unwrap();
        assert_eq!(&query[12..22], b"\x04e621\x03net\0");
        assert_eq!(read_u16(&query, 22).unwrap(), HTTPS_RR);
    }
}
