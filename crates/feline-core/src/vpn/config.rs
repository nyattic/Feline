use std::net::IpAddr;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WgConfig {
    pub interface: WgInterface,
    pub peer: WgPeer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WgInterface {
    pub private_key: String,
    pub addresses: Vec<IpCidr>,
    #[serde(default)]
    pub dns: Vec<IpAddr>,
    #[serde(default)]
    pub mtu: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WgPeer {
    pub public_key: String,
    #[serde(default)]
    pub preshared_key: Option<String>,
    pub allowed_ips: Vec<IpCidr>,
    pub endpoint: String,
    #[serde(default)]
    pub persistent_keepalive: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpCidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl FromStr for IpCidr {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (addr_str, prefix) = match s.split_once('/') {
            Some((a, p)) => {
                let prefix: u8 = p.trim().parse().with_context(|| {
                    format!("invalid CIDR prefix `{p}` in `{s}` (expected 0-128)")
                })?;
                (a.trim(), prefix)
            }
            None => (s, default_prefix_for(s)?),
        };
        let addr: IpAddr = addr_str
            .parse()
            .with_context(|| format!("invalid IP address `{addr_str}` in `{s}`"))?;
        let max = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix > max {
            bail!("CIDR prefix /{prefix} exceeds maximum /{max} for `{addr_str}`");
        }
        Ok(IpCidr { addr, prefix })
    }
}

fn default_prefix_for(s: &str) -> Result<u8> {
    let addr: IpAddr = s
        .parse()
        .with_context(|| format!("invalid IP address `{s}`"))?;
    Ok(match addr {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Section {
    Interface,
    Peer,
}

#[derive(Default)]
struct InterfaceBuilder {
    private_key: Option<String>,
    addresses: Vec<IpCidr>,
    dns: Vec<IpAddr>,
    mtu: Option<u16>,
}

impl InterfaceBuilder {
    fn build(self) -> Result<WgInterface> {
        let private_key = self
            .private_key
            .ok_or_else(|| anyhow!("[Interface] is missing PrivateKey"))?;
        validate_wg_key(&private_key).context("PrivateKey")?;
        if self.addresses.is_empty() {
            bail!("[Interface] is missing Address");
        }
        Ok(WgInterface {
            private_key,
            addresses: self.addresses,
            dns: self.dns,
            mtu: self.mtu,
        })
    }
}

#[derive(Default)]
struct PeerBuilder {
    public_key: Option<String>,
    preshared_key: Option<String>,
    allowed_ips: Vec<IpCidr>,
    endpoint: Option<String>,
    persistent_keepalive: Option<u16>,
}

impl PeerBuilder {
    fn build(self) -> Result<WgPeer> {
        let public_key = self
            .public_key
            .ok_or_else(|| anyhow!("[Peer] is missing PublicKey"))?;
        validate_wg_key(&public_key).context("PublicKey")?;
        if let Some(psk) = &self.preshared_key {
            validate_wg_key(psk).context("PresharedKey")?;
        }
        let endpoint = self
            .endpoint
            .ok_or_else(|| anyhow!("[Peer] is missing Endpoint"))?;
        validate_endpoint(&endpoint)?;
        if self.allowed_ips.is_empty() {
            bail!("[Peer] is missing AllowedIPs");
        }
        Ok(WgPeer {
            public_key,
            preshared_key: self.preshared_key,
            allowed_ips: self.allowed_ips,
            endpoint,
            persistent_keepalive: self.persistent_keepalive,
        })
    }
}

pub fn parse(input: &str) -> Result<WgConfig> {
    let mut section: Option<Section> = None;
    let mut interface = InterfaceBuilder::default();
    let mut peer = PeerBuilder::default();
    let mut saw_interface = false;
    let mut saw_peer = false;

    for (idx, raw) in input.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = section_header(line) {
            match name.to_ascii_lowercase().as_str() {
                "interface" => {
                    if saw_interface {
                        bail!("duplicate [Interface] section at line {line_no}");
                    }
                    saw_interface = true;
                    section = Some(Section::Interface);
                }
                "peer" => {
                    if saw_peer {
                        bail!("multiple [Peer] sections are not supported (line {line_no})");
                    }
                    saw_peer = true;
                    section = Some(Section::Peer);
                }
                other => bail!("unknown section [{other}] at line {line_no}"),
            }
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("expected `key = value` at line {line_no}: `{line}`"))?;
        let key_norm = key.trim().to_ascii_lowercase();
        let value = value.trim();

        match section {
            Some(Section::Interface) => apply_interface(&mut interface, &key_norm, value, line_no)?,
            Some(Section::Peer) => apply_peer(&mut peer, &key_norm, value, line_no)?,
            None => bail!(
                "key `{key}` at line {line_no} appears before any [Interface] or [Peer] section"
            ),
        }
    }

    if !saw_interface {
        bail!("config is missing [Interface] section");
    }
    if !saw_peer {
        bail!("config is missing [Peer] section");
    }

    Ok(WgConfig {
        interface: interface.build()?,
        peer: peer.build()?,
    })
}

fn section_header(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
        Some(line[1..line.len() - 1].trim())
    } else {
        None
    }
}

fn strip_comment(line: &str) -> &str {
    let comment_pos = line.find(['#', ';']).unwrap_or(line.len());
    &line[..comment_pos]
}

fn apply_interface(
    builder: &mut InterfaceBuilder,
    key: &str,
    value: &str,
    line_no: usize,
) -> Result<()> {
    match key {
        "privatekey" => {
            builder.private_key = Some(value.to_string());
        }
        "address" => {
            for item in split_list(value) {
                let cidr: IpCidr = item
                    .parse()
                    .with_context(|| format!("Address at line {line_no}"))?;
                builder.addresses.push(cidr);
            }
        }
        "dns" => {
            for item in split_list(value) {
                let ip: IpAddr = item
                    .parse()
                    .with_context(|| format!("DNS at line {line_no}: `{item}`"))?;
                builder.dns.push(ip);
            }
        }
        "mtu" => {
            let mtu: u16 = value
                .parse()
                .with_context(|| format!("MTU at line {line_no}: `{value}`"))?;
            if !(576..=9000).contains(&mtu) {
                bail!("MTU {mtu} at line {line_no} is outside the supported range 576-9000");
            }
            builder.mtu = Some(mtu);
        }
        "table" | "preup" | "postup" | "predown" | "postdown" | "saveconfig" | "fwmark" => {}
        other => bail!("unknown [Interface] key `{other}` at line {line_no}"),
    }
    Ok(())
}

fn apply_peer(builder: &mut PeerBuilder, key: &str, value: &str, line_no: usize) -> Result<()> {
    match key {
        "publickey" => {
            builder.public_key = Some(value.to_string());
        }
        "presharedkey" => {
            if !value.is_empty() {
                builder.preshared_key = Some(value.to_string());
            }
        }
        "allowedips" => {
            for item in split_list(value) {
                let cidr: IpCidr = item
                    .parse()
                    .with_context(|| format!("AllowedIPs at line {line_no}"))?;
                builder.allowed_ips.push(cidr);
            }
        }
        "endpoint" => {
            builder.endpoint = Some(value.to_string());
        }
        "persistentkeepalive" => {
            let secs: u16 = value
                .parse()
                .with_context(|| format!("PersistentKeepalive at line {line_no}: `{value}`"))?;
            builder.persistent_keepalive = Some(secs);
        }
        other => bail!("unknown [Peer] key `{other}` at line {line_no}"),
    }
    Ok(())
}

fn split_list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn validate_wg_key(key: &str) -> Result<()> {
    let bytes = BASE64
        .decode(key.trim().as_bytes())
        .context("not valid base64")?;
    if bytes.len() != 32 {
        bail!(
            "expected 32-byte WireGuard key, got {} bytes after base64 decode",
            bytes.len()
        );
    }
    Ok(())
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let (host, port_str) = if let Some(rest) = endpoint.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| anyhow!("Endpoint `{endpoint}` has unclosed `[`"))?;
        let port_str = tail
            .strip_prefix(':')
            .ok_or_else(|| anyhow!("Endpoint `{endpoint}` is missing `:port` after `]`"))?;
        (host, port_str)
    } else {
        endpoint
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("Endpoint `{endpoint}` is missing `:port`"))?
    };
    if host.is_empty() {
        bail!("Endpoint `{endpoint}` has empty host");
    }
    let _port: u16 = port_str
        .parse()
        .with_context(|| format!("Endpoint `{endpoint}` has invalid port `{port_str}`"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    const KEY_B: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";
    const KEY_C: &str = "QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8=";
    const KEY_D: &str = "YGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn8=";

    fn mullvad_conf() -> String {
        format!(
            "\
[Interface]
# Device: feline
PrivateKey = {KEY_A}
Address = 10.64.123.45/32,fc00:bbbb:bbbb:bb01::1:abcd/128
DNS = 10.64.0.1

[Peer]
PublicKey = {KEY_B}
AllowedIPs = 0.0.0.0/0,::/0
Endpoint = jp-tyo-wg-001.mullvad.net:51820
"
        )
    }

    fn proton_conf() -> String {
        format!(
            "\
[Interface]
# Key for Feline
# Bouncing = 1
# NetShield = 0
PrivateKey = {KEY_C}
Address = 10.2.0.2/32

[Peer]
# JP#42
PublicKey = {KEY_D}
AllowedIPs = 0.0.0.0/0
Endpoint = 203.0.113.42:51820
PersistentKeepalive = 25
"
        )
    }

    #[test]
    fn parses_mullvad_style_config() {
        let cfg = parse(&mullvad_conf()).expect("parse Mullvad config");
        assert_eq!(cfg.interface.addresses.len(), 2);
        assert_eq!(cfg.interface.addresses[0].prefix, 32);
        assert_eq!(cfg.interface.addresses[1].prefix, 128);
        assert_eq!(cfg.interface.dns.len(), 1);
        assert_eq!(cfg.peer.endpoint, "jp-tyo-wg-001.mullvad.net:51820");
        assert_eq!(cfg.peer.allowed_ips.len(), 2);
        assert!(cfg.peer.persistent_keepalive.is_none());
    }

    #[test]
    fn parses_proton_style_config() {
        let cfg = parse(&proton_conf()).expect("parse Proton config");
        assert!(cfg.interface.dns.is_empty());
        assert_eq!(cfg.peer.persistent_keepalive, Some(25));
        assert_eq!(cfg.peer.endpoint, "203.0.113.42:51820");
    }

    #[test]
    fn rejects_missing_interface() {
        let conf = format!(
            "[Peer]\nPublicKey = {KEY_B}\nAllowedIPs = 0.0.0.0/0\nEndpoint = 1.2.3.4:51820\n"
        );
        let err = format!("{:#}", parse(&conf).unwrap_err());
        assert!(err.contains("[Interface]"), "got: {err}");
    }

    #[test]
    fn rejects_missing_peer() {
        let conf = format!("[Interface]\nPrivateKey = {KEY_A}\nAddress = 10.0.0.1/32\n");
        let err = format!("{:#}", parse(&conf).unwrap_err());
        assert!(err.contains("[Peer]"), "got: {err}");
    }

    #[test]
    fn rejects_invalid_base64_key() {
        let conf = format!(
            "[Interface]\nPrivateKey = not-base64!!\nAddress = 10.0.0.1/32\n\n[Peer]\nPublicKey = {KEY_B}\nAllowedIPs = 0.0.0.0/0\nEndpoint = 1.2.3.4:51820\n"
        );
        let err = format!("{:#}", parse(&conf).unwrap_err());
        assert!(err.contains("PrivateKey"), "got: {err}");
        assert!(err.contains("base64"), "got: {err}");
    }

    #[test]
    fn rejects_wrong_length_key() {
        let conf = format!(
            "[Interface]\nPrivateKey = dGVzdA==\nAddress = 10.0.0.1/32\n\n[Peer]\nPublicKey = {KEY_B}\nAllowedIPs = 0.0.0.0/0\nEndpoint = 1.2.3.4:51820\n"
        );
        let err = format!("{:#}", parse(&conf).unwrap_err());
        assert!(err.contains("32-byte"), "got: {err}");
    }

    #[test]
    fn rejects_multiple_peers() {
        let conf = format!(
            "\
[Interface]
PrivateKey = {KEY_A}
Address = 10.0.0.1/32

[Peer]
PublicKey = {KEY_B}
AllowedIPs = 0.0.0.0/0
Endpoint = 1.2.3.4:51820

[Peer]
PublicKey = {KEY_C}
AllowedIPs = 10.0.0.0/8
Endpoint = 5.6.7.8:51820
"
        );
        let err = format!("{:#}", parse(&conf).unwrap_err());
        assert!(err.contains("multiple [Peer]"), "got: {err}");
    }

    #[test]
    fn ignores_comments_and_wg_quick_directives() {
        let conf = format!(
            "\
[Interface]
# This is a comment
; And this is also a comment
PrivateKey = {KEY_A}
Address = 10.0.0.1/32
PostUp = iptables -A FORWARD -i %i -j ACCEPT
PreDown = iptables -D FORWARD -i %i -j ACCEPT
Table = off

[Peer]
PublicKey = {KEY_B} # inline
AllowedIPs = 0.0.0.0/0
Endpoint = 1.2.3.4:51820
"
        );
        let cfg = parse(&conf).expect("parse with comments and wg-quick directives");
        assert_eq!(cfg.peer.endpoint, "1.2.3.4:51820");
    }

    #[test]
    fn endpoint_supports_ipv6_bracketed_form() {
        let conf = format!(
            "\
[Interface]
PrivateKey = {KEY_A}
Address = 10.0.0.1/32

[Peer]
PublicKey = {KEY_B}
AllowedIPs = 0.0.0.0/0
Endpoint = [2001:db8::1]:51820
"
        );
        let cfg = parse(&conf).expect("parse IPv6 endpoint");
        assert_eq!(cfg.peer.endpoint, "[2001:db8::1]:51820");
    }

    #[test]
    fn ipcidr_defaults_prefix_for_bare_address() {
        let v4: IpCidr = "10.0.0.1".parse().expect("parse bare IPv4");
        assert_eq!(v4.prefix, 32);
        let v6: IpCidr = "fe80::1".parse().expect("parse bare IPv6");
        assert_eq!(v6.prefix, 128);
    }

    #[test]
    fn ipcidr_rejects_oversized_prefix() {
        let err: anyhow::Error = "10.0.0.0/40".parse::<IpCidr>().unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("exceeds maximum"), "got: {chain}");
    }

    #[test]
    fn list_split_handles_commas_and_whitespace() {
        let items: Vec<&str> = split_list("1.1.1.1,  2.2.2.2  3.3.3.3").collect();
        assert_eq!(items, vec!["1.1.1.1", "2.2.2.2", "3.3.3.3"]);
    }

    #[test]
    fn round_trip_through_serde_json() {
        let parsed = parse(&mullvad_conf()).unwrap();
        let json = serde_json::to_string(&parsed).unwrap();
        let back: WgConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, back);
    }
}
