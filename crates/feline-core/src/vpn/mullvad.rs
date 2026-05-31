use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use boringtun::x25519::{PublicKey, StaticSecret};
use rand::RngExt;
use serde::{Deserialize, Serialize};

use super::config::{IpCidr, WgConfig, WgInterface, WgPeer};

const TOKEN_URL: &str = "https://api.mullvad.net/auth/v1/token";
const DEVICES_URL: &str = "https://api.mullvad.net/accounts/v1/devices";
const RELAYS_URL: &str = "https://api.mullvad.net/app/v1/relays";
const MULLVAD_DNS: &str = "10.64.0.1";
const WG_PORT: u16 = 51820;
const KEEPALIVE_SECS: u16 = 25;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

const USER_AGENT_VALUE: &str = concat!(
    "Feline/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/nyattic/Feline)"
);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MullvadProfile {
    pub account_number: String,
    pub private_key: String,
    pub device_id: String,
    pub device_name: String,
    pub addresses: Vec<IpCidr>,
    pub country_code: String,
    pub country_name: String,
    pub city_code: String,
    pub city_name: String,
}

pub struct RegisteredDevice {
    pub id: String,
    pub name: String,
    pub addresses: Vec<IpCidr>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MullvadCity {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MullvadCountry {
    pub code: String,
    pub name: String,
    pub cities: Vec<MullvadCity>,
}

#[derive(Debug, Clone)]
pub struct ChosenRelay {
    pub public_key: String,
    pub endpoint: String,
    pub country_code: String,
    pub country_name: String,
    pub city_code: String,
    pub city_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelayList {
    #[serde(default)]
    locations: BTreeMap<String, RelayLocation>,
    wireguard: WireguardSection,
}

#[derive(Debug, Clone, Deserialize)]
struct RelayLocation {
    country: String,
    city: String,
}

#[derive(Debug, Clone, Deserialize)]
struct WireguardSection {
    #[serde(default)]
    relays: Vec<WireguardRelay>,
}

#[derive(Debug, Clone, Deserialize)]
struct WireguardRelay {
    location: String,
    #[serde(default)]
    active: bool,
    public_key: String,
    ipv4_addr_in: String,
    #[serde(default)]
    weight: u32,
}

pub fn normalize_account(input: &str) -> Result<String> {
    let digits: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.is_empty() {
        bail!("enter your Mullvad account number");
    }
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("a Mullvad account number contains only digits");
    }
    if digits.len() != 16 {
        bail!("a Mullvad account number is 16 digits long");
    }
    Ok(digits)
}

pub fn generate_keypair() -> (String, String) {
    let mut private_key = [0u8; 32];
    rand::rng().fill(&mut private_key);
    let secret = StaticSecret::from(private_key);
    let public = PublicKey::from(&secret);
    (
        BASE64.encode(secret.to_bytes()),
        BASE64.encode(public.to_bytes()),
    )
}

fn client() -> Result<wreq::Client> {
    wreq::Client::builder()
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT_VALUE)
        .build()
        .context("build Mullvad API client")
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    account_number: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

pub async fn fetch_token(account_number: &str) -> Result<String> {
    let http = client()?;
    let resp = http
        .post(TOKEN_URL)
        .json(&TokenRequest { account_number })
        .send()
        .await
        .context("contact Mullvad to sign in")?;

    let status = resp.status();
    if status.as_u16() == 400 || status.as_u16() == 401 || status.as_u16() == 404 {
        bail!("that Mullvad account number was not accepted");
    }
    if !status.is_success() {
        let body = body_snippet(resp).await;
        bail!("Mullvad sign-in failed: HTTP {status} {body}");
    }

    let parsed: TokenResponse = resp.json().await.context("decode Mullvad token response")?;
    Ok(parsed.access_token)
}

#[derive(Serialize)]
struct DeviceRequest<'a> {
    pubkey: &'a str,
    hijack_dns: bool,
}

#[derive(Deserialize)]
struct DeviceResponse {
    id: String,
    name: String,
    ipv4_address: String,
    ipv6_address: String,
}

#[derive(Deserialize)]
struct DeviceSummary {
    id: String,
    pubkey: String,
}

pub async fn register_device(token: &str, public_key: &str) -> Result<RegisteredDevice> {
    let http = client()?;
    let resp = http
        .post(DEVICES_URL)
        .bearer_auth(token)
        .json(&DeviceRequest {
            pubkey: public_key,
            hijack_dns: false,
        })
        .send()
        .await
        .context("register device with Mullvad")?;

    let status = resp.status();
    if !status.is_success() {
        let body = body_snippet(resp).await;
        if body.to_ascii_uppercase().contains("MAX_DEVICES") {
            bail!(
                "this Mullvad account already has its maximum of 5 devices; remove one from your Mullvad account and try again"
            );
        }
        bail!("Mullvad device registration failed: HTTP {status} {body}");
    }

    let device: DeviceResponse = resp.json().await.context("decode Mullvad device response")?;
    let addresses = parse_addresses(&device.ipv4_address, &device.ipv6_address)?;
    Ok(RegisteredDevice {
        id: device.id,
        name: device.name,
        addresses,
    })
}

pub async fn device_exists(token: &str, device_id: &str, public_key: &str) -> Result<bool> {
    let http = client()?;
    let resp = http
        .get(DEVICES_URL)
        .bearer_auth(token)
        .send()
        .await
        .context("list Mullvad devices")?;

    if !resp.status().is_success() {
        return Ok(false);
    }

    let devices: Vec<DeviceSummary> = resp.json().await.context("decode Mullvad device list")?;
    Ok(devices
        .iter()
        .any(|d| d.id == device_id && d.pubkey == public_key))
}

pub async fn delete_device(token: &str, device_id: &str) -> Result<()> {
    let http = client()?;
    let resp = http
        .delete(format!("{DEVICES_URL}/{device_id}"))
        .bearer_auth(token)
        .send()
        .await
        .context("remove device from Mullvad")?;

    let status = resp.status();
    if status.is_success() || status.as_u16() == 404 {
        return Ok(());
    }
    let body = body_snippet(resp).await;
    bail!("Mullvad device removal failed: HTTP {status} {body}")
}

pub async fn fetch_relays() -> Result<RelayList> {
    let http = client()?;
    let resp = http
        .get(RELAYS_URL)
        .send()
        .await
        .context("download Mullvad relay list")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = body_snippet(resp).await;
        bail!("Mullvad relay list failed: HTTP {status} {body}");
    }

    resp.json().await.context("decode Mullvad relay list")
}

impl RelayList {
    pub fn locations_tree(&self) -> Vec<MullvadCountry> {
        let mut countries: BTreeMap<String, (String, BTreeMap<String, String>)> = BTreeMap::new();

        for relay in self.active_relays() {
            let Some(location) = self.locations.get(&relay.location) else {
                continue;
            };
            let country_code = country_of(&relay.location);
            let entry = countries
                .entry(country_code)
                .or_insert_with(|| (location.country.clone(), BTreeMap::new()));
            entry
                .1
                .entry(relay.location.clone())
                .or_insert_with(|| location.city.clone());
        }

        let mut out: Vec<MullvadCountry> = countries
            .into_iter()
            .map(|(code, (name, cities))| MullvadCountry {
                code,
                name,
                cities: cities
                    .into_iter()
                    .map(|(code, name)| MullvadCity { code, name })
                    .collect(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        for country in &mut out {
            country.cities.sort_by(|a, b| a.name.cmp(&b.name));
        }
        out
    }

    pub fn choose(&self, city_code: &str) -> Option<ChosenRelay> {
        let location = self.locations.get(city_code)?;
        let relay = self
            .active_relays()
            .filter(|r| r.location == city_code)
            .max_by_key(|r| r.weight)?;
        Some(ChosenRelay {
            public_key: relay.public_key.clone(),
            endpoint: format!("{}:{WG_PORT}", relay.ipv4_addr_in),
            country_code: country_of(city_code),
            country_name: location.country.clone(),
            city_code: city_code.to_string(),
            city_name: location.city.clone(),
        })
    }

    pub fn default_choice(&self) -> Option<ChosenRelay> {
        let tree = self.locations_tree();
        let preferred = tree.iter().find(|c| c.code == "jp").or_else(|| tree.first());
        let city_code = preferred?.cities.first()?.code.clone();
        self.choose(&city_code)
    }

    fn active_relays(&self) -> impl Iterator<Item = &WireguardRelay> {
        self.wireguard.relays.iter().filter(|r| r.active)
    }
}

pub fn build_config(profile: &MullvadProfile, relay: &ChosenRelay) -> Result<WgConfig> {
    let dns = std::net::IpAddr::from_str(MULLVAD_DNS).context("parse Mullvad DNS address")?;
    let allowed_ips = vec![
        IpCidr::from_str("0.0.0.0/0").context("build AllowedIPs")?,
        IpCidr::from_str("::/0").context("build AllowedIPs")?,
    ];
    Ok(WgConfig {
        interface: WgInterface {
            private_key: profile.private_key.clone(),
            addresses: profile.addresses.clone(),
            dns: vec![dns],
            mtu: None,
        },
        peer: WgPeer {
            public_key: relay.public_key.clone(),
            preshared_key: None,
            allowed_ips,
            endpoint: relay.endpoint.clone(),
            persistent_keepalive: Some(KEEPALIVE_SECS),
        },
    })
}

pub fn mask_account(account: &str) -> String {
    let visible: String = account.chars().rev().take(4).collect::<String>();
    let visible: String = visible.chars().rev().collect();
    format!("•••• •••• •••• {visible}")
}

fn parse_addresses(ipv4: &str, ipv6: &str) -> Result<Vec<IpCidr>> {
    let mut out = Vec::new();
    out.push(IpCidr::from_str(ipv4).context("parse assigned IPv4 address")?);
    if !ipv6.trim().is_empty() {
        out.push(IpCidr::from_str(ipv6).context("parse assigned IPv6 address")?);
    }
    Ok(out)
}

fn country_of(city_code: &str) -> String {
    city_code.split('-').next().unwrap_or(city_code).to_string()
}

async fn body_snippet(resp: wreq::Response) -> String {
    let body = resp.text().await.unwrap_or_default();
    let mut out = body.trim().replace('\n', " ");
    if out.len() > 200 {
        out.truncate(200);
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAYS_JSON: &str = r#"{
        "locations": {
            "jp-tyo": {"country": "Japan", "city": "Tokyo", "latitude": 35.6, "longitude": 139.6},
            "us-nyc": {"country": "USA", "city": "New York", "latitude": 40.7, "longitude": -74.0},
            "se-got": {"country": "Sweden", "city": "Gothenburg", "latitude": 57.7, "longitude": 11.9}
        },
        "openvpn": {"ports": []},
        "wireguard": {
            "relays": [
                {"hostname": "jp-tyo-wg-001", "location": "jp-tyo", "active": true, "owned": true, "provider": "M", "ipv4_addr_in": "203.0.113.1", "ipv6_addr_in": "::1", "public_key": "JPKEY1", "weight": 100},
                {"hostname": "jp-tyo-wg-002", "location": "jp-tyo", "active": true, "owned": true, "provider": "M", "ipv4_addr_in": "203.0.113.2", "ipv6_addr_in": "::2", "public_key": "JPKEY2", "weight": 500},
                {"hostname": "us-nyc-wg-001", "location": "us-nyc", "active": true, "owned": false, "provider": "X", "ipv4_addr_in": "198.51.100.1", "ipv6_addr_in": "::3", "public_key": "USKEY1", "weight": 100},
                {"hostname": "se-got-wg-001", "location": "se-got", "active": false, "owned": true, "provider": "M", "ipv4_addr_in": "192.0.2.1", "ipv6_addr_in": "::4", "public_key": "SEKEY1", "weight": 100}
            ],
            "ports": []
        }
    }"#;

    fn relays() -> RelayList {
        serde_json::from_str(RELAYS_JSON).expect("parse relay fixture")
    }

    fn profile() -> MullvadProfile {
        MullvadProfile {
            account_number: "1234123412341234".to_string(),
            private_key: "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=".to_string(),
            device_id: "dev-1".to_string(),
            device_name: "happy-cat".to_string(),
            addresses: vec![IpCidr::from_str("10.64.0.2/32").unwrap()],
            country_code: "jp".to_string(),
            country_name: "Japan".to_string(),
            city_code: "jp-tyo".to_string(),
            city_name: "Tokyo".to_string(),
        }
    }

    #[test]
    fn normalize_strips_spaces_and_validates() {
        assert_eq!(
            normalize_account("1234 5678 9012 3456").unwrap(),
            "1234567890123456"
        );
        assert!(normalize_account("1234").is_err());
        assert!(normalize_account("abcd5678abcd5678").is_err());
    }

    #[test]
    fn keypair_is_valid_base64_32_bytes() {
        let (secret, public) = generate_keypair();
        assert_eq!(BASE64.decode(secret).unwrap().len(), 32);
        assert_eq!(BASE64.decode(public).unwrap().len(), 32);
    }

    #[test]
    fn tree_excludes_inactive_and_sorts() {
        let tree = relays().locations_tree();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].name, "Japan");
        assert_eq!(tree[1].name, "USA");
        assert!(!tree.iter().any(|c| c.name == "Sweden"));
    }

    #[test]
    fn choose_picks_highest_weight() {
        let chosen = relays().choose("jp-tyo").expect("choose jp-tyo");
        assert_eq!(chosen.public_key, "JPKEY2");
        assert_eq!(chosen.endpoint, "203.0.113.2:51820");
        assert_eq!(chosen.country_code, "jp");
        assert_eq!(chosen.city_name, "Tokyo");
    }

    #[test]
    fn default_choice_prefers_japan() {
        let chosen = relays().default_choice().expect("default choice");
        assert_eq!(chosen.country_code, "jp");
    }

    #[test]
    fn build_config_sets_full_tunnel_and_dns() {
        let chosen = relays().choose("jp-tyo").unwrap();
        let cfg = build_config(&profile(), &chosen).unwrap();
        assert_eq!(cfg.peer.endpoint, "203.0.113.2:51820");
        assert_eq!(cfg.peer.public_key, "JPKEY2");
        assert_eq!(cfg.peer.allowed_ips.len(), 2);
        assert_eq!(cfg.peer.persistent_keepalive, Some(25));
        assert_eq!(cfg.interface.dns.len(), 1);
        assert_eq!(cfg.interface.dns[0].to_string(), "10.64.0.1");
    }

    #[test]
    fn mask_account_shows_last_four() {
        assert_eq!(mask_account("1234567890123456"), "•••• •••• •••• 3456");
    }
}
