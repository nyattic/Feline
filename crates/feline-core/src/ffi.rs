use crate::Site;
use crate::credentials::Credentials;
use crate::e621::client::Client;
use crate::e621::rate_limit::new_api_limiter;
use crate::media_cache;
use crate::vpn;
use crate::vpn::config::IpCidr;
use crate::vpn::mullvad;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

#[derive(uniffi::Record)]
pub struct FfiCredentials {
    pub username: String,
    pub api_key: String,
}

impl From<FfiCredentials> for Credentials {
    fn from(c: FfiCredentials) -> Self {
        Credentials {
            username: c.username,
            api_key: c.api_key,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy)]
pub enum FfiSite {
    E621,
    E926,
}

impl From<FfiSite> for Site {
    fn from(s: FfiSite) -> Self {
        match s {
            FfiSite::E621 => Site::E621,
            FfiSite::E926 => Site::E926,
        }
    }
}

#[derive(uniffi::Record)]
pub struct FfiResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(uniffi::Record)]
pub struct FfiQueryParam {
    pub key: String,
    pub value: String,
}

#[derive(uniffi::Record, Clone)]
pub struct FfiMullvadProfile {
    pub account_number: String,
    pub private_key: String,
    pub device_id: String,
    pub device_name: String,
    pub addresses: Vec<String>,
    pub country_code: String,
    pub country_name: String,
    pub city_code: String,
    pub city_name: String,
}

#[derive(uniffi::Record)]
pub struct FfiMullvadCity {
    pub code: String,
    pub name: String,
}

#[derive(uniffi::Record)]
pub struct FfiMullvadCountry {
    pub code: String,
    pub name: String,
    pub cities: Vec<FfiMullvadCity>,
}

fn profile_to_ffi(p: &mullvad::MullvadProfile) -> FfiMullvadProfile {
    FfiMullvadProfile {
        account_number: p.account_number.clone(),
        private_key: p.private_key.clone(),
        device_id: p.device_id.clone(),
        device_name: p.device_name.clone(),
        addresses: p
            .addresses
            .iter()
            .map(|c| format!("{}/{}", c.addr, c.prefix))
            .collect(),
        country_code: p.country_code.clone(),
        country_name: p.country_name.clone(),
        city_code: p.city_code.clone(),
        city_name: p.city_name.clone(),
    }
}

fn ffi_to_profile(p: FfiMullvadProfile) -> anyhow::Result<mullvad::MullvadProfile> {
    let mut addresses = Vec::with_capacity(p.addresses.len());
    for raw in &p.addresses {
        addresses.push(IpCidr::from_str(raw)?);
    }
    Ok(mullvad::MullvadProfile {
        account_number: p.account_number,
        private_key: p.private_key,
        device_id: p.device_id,
        device_name: p.device_name,
        addresses,
        country_code: p.country_code,
        country_name: p.country_name,
        city_code: p.city_code,
        city_name: p.city_name,
    })
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("session expired: {0}")]
    SessionExpired(String),
}

impl From<anyhow::Error> for FfiError {
    fn from(e: anyhow::Error) -> Self {
        if e.downcast_ref::<mullvad::MullvadError>().is_some() {
            return FfiError::SessionExpired("Mullvad session expired".into());
        }

        let s = format!("{e:#}");
        if s.contains("path must start with '/'") || s.contains("unsupported HTTP method") {
            FfiError::InvalidArgument(s)
        } else {
            FfiError::Network(s)
        }
    }
}

#[derive(uniffi::Object)]
pub struct E621Core {
    inner: Client,
    file_root: Option<PathBuf>,
}

#[uniffi::export(async_runtime = "tokio")]
impl E621Core {
    #[uniffi::constructor]
    pub async fn new(
        site: FfiSite,
        credentials: Option<FfiCredentials>,
        proxy_url: Option<String>,
        cache_dir: Option<String>,
    ) -> Result<Arc<Self>, FfiError> {
        let creds = credentials.map(Into::into);
        let limiter = new_api_limiter();
        let file_root = cache_dir
            .as_deref()
            .map(prepare_file_root)
            .transpose()
            .map_err(|e| FfiError::InvalidArgument(format!("{e:#}")))?;
        let mut client = Client::with_limiter(site.into(), creds, limiter, proxy_url)
            .await
            .map_err(FfiError::from)?;
        client.set_cache_dir(file_root.clone());
        Ok(Arc::new(Self {
            inner: client,
            file_root,
        }))
    }

    pub async fn request(
        &self,
        method: String,
        path: String,
        query: Vec<FfiQueryParam>,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    ) -> Result<FfiResponse, FfiError> {
        let q: Vec<(String, String)> = query.into_iter().map(|p| (p.key, p.value)).collect();
        let resp = self
            .inner
            .raw_request(&method, &path, &q, body, content_type.as_deref())
            .await
            .map_err(FfiError::from)?;
        Ok(FfiResponse {
            status: resp.status,
            body: resp.body,
        })
    }

    pub async fn fetch_media(&self, url: String) -> Result<FfiResponse, FfiError> {
        let resp = self.inner.fetch_media(&url).await.map_err(FfiError::from)?;
        Ok(FfiResponse {
            status: resp.status,
            body: resp.body,
        })
    }

    pub async fn download_to_file(&self, url: String, dest_path: String) -> Result<u16, FfiError> {
        let dest_path = scoped_file_path(self.file_root.as_deref(), &dest_path)?;
        let dest_path = dest_path.to_string_lossy().into_owned();
        self.inner
            .download_to_file(&url, &dest_path)
            .await
            .map_err(FfiError::from)
    }
}

fn prepare_file_root(raw: &str) -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(raw);
    media_cache::prepare_dir(&dir)?;
    dir.canonicalize()
        .map_err(anyhow::Error::from)
        .map_err(|e| e.context("canonicalize media cache dir"))
}

fn require_prepared_cache_dir(raw: &str) -> Result<PathBuf, FfiError> {
    let dir = PathBuf::from(raw)
        .canonicalize()
        .map_err(|e| FfiError::InvalidArgument(format!("invalid media cache dir: {e}")))?;
    if !media_cache::is_prepared_dir(&dir) {
        return Err(FfiError::InvalidArgument(
            "media cache dir is not managed by Feline".into(),
        ));
    }
    Ok(dir)
}

fn scoped_file_path(root: Option<&Path>, raw_path: &str) -> Result<PathBuf, FfiError> {
    let Some(root) = root else {
        return Err(FfiError::InvalidArgument(
            "download_to_file requires a configured cache_dir".into(),
        ));
    };
    let path = PathBuf::from(raw_path);
    if path.file_name().is_none() {
        return Err(FfiError::InvalidArgument(
            "destination path must include a file name".into(),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        FfiError::InvalidArgument("destination path must include a parent directory".into())
    })?;
    let parent = parent
        .canonicalize()
        .map_err(|e| FfiError::InvalidArgument(format!("invalid destination parent: {e}")))?;
    if !parent.starts_with(root) {
        return Err(FfiError::InvalidArgument(
            "destination path must be inside the configured cache_dir".into(),
        ));
    }
    Ok(path)
}

#[derive(uniffi::Object)]
pub struct FelineVpn {
    inner: std::sync::Mutex<Option<vpn::VpnHandle>>,
    last_config: std::sync::Mutex<Option<vpn::WgConfig>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl FelineVpn {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: std::sync::Mutex::new(None),
            last_config: std::sync::Mutex::new(None),
        })
    }

    pub async fn enable(&self, config_text: String) -> Result<(), FfiError> {
        let cfg =
            vpn::parse(&config_text).map_err(|e| FfiError::InvalidArgument(format!("{e:#}")))?;
        self.swap_handle(cfg).await
    }

    pub async fn enable_mullvad(
        &self,
        profile: FfiMullvadProfile,
        city_code: Option<String>,
    ) -> Result<(), FfiError> {
        let profile = ffi_to_profile(profile).map_err(FfiError::from)?;
        mullvad::ensure_device_active(&profile.account_number, &profile.device_id)
            .await
            .map_err(FfiError::from)?;
        let relays = mullvad::fetch_relays().await.map_err(FfiError::from)?;
        let chosen = match city_code {
            Some(code) => relays.choose(&code),
            None => relays
                .choose(&profile.city_code)
                .or_else(|| relays.default_choice()),
        }
        .ok_or_else(|| FfiError::Network("the selected Mullvad city is unavailable".into()))?;
        let cfg = mullvad::build_config(&profile, &chosen).map_err(FfiError::from)?;
        self.swap_handle(cfg).await
    }

    pub async fn reconnect(&self) -> Result<(), FfiError> {
        let cfg = self
            .last_config
            .lock()
            .expect("vpn config poisoned")
            .clone()
            .ok_or_else(|| FfiError::Network("no VPN configuration to reconnect".into()))?;
        let handle = vpn::VpnHandle::start(cfg).await.map_err(FfiError::from)?;
        let previous = self
            .inner
            .lock()
            .expect("vpn handle poisoned")
            .replace(handle);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
        Ok(())
    }

    pub async fn disable(&self) {
        *self.last_config.lock().expect("vpn config poisoned") = None;
        let previous = self.inner.lock().expect("vpn handle poisoned").take();
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.lock().expect("vpn handle poisoned").is_some()
    }

    pub fn is_healthy(&self) -> bool {
        self.inner
            .lock()
            .expect("vpn handle poisoned")
            .as_ref()
            .is_some_and(|h| h.is_healthy())
    }

    pub fn proxy_url(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("vpn handle poisoned")
            .as_ref()
            .and_then(|h| h.proxy_url())
    }
}

impl FelineVpn {
    async fn swap_handle(&self, cfg: vpn::WgConfig) -> Result<(), FfiError> {
        let handle = vpn::VpnHandle::start(cfg.clone())
            .await
            .map_err(FfiError::from)?;
        *self.last_config.lock().expect("vpn config poisoned") = Some(cfg);
        let previous = self
            .inner
            .lock()
            .expect("vpn handle poisoned")
            .replace(handle);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
        Ok(())
    }
}

#[uniffi::export]
pub fn validate_wg_config(config_text: String) -> Result<(), FfiError> {
    vpn::parse(&config_text)
        .map(|_| ())
        .map_err(|e| FfiError::InvalidArgument(format!("{e:#}")))
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn mullvad_sign_in(account_number: String) -> Result<FfiMullvadProfile, FfiError> {
    let account = mullvad::normalize_account(&account_number).map_err(FfiError::from)?;
    let token = mullvad::fetch_token(&account)
        .await
        .map_err(FfiError::from)?;
    let (private_key, public_key) = mullvad::generate_keypair();
    let device = mullvad::register_device(&token, &public_key)
        .await
        .map_err(FfiError::from)?;

    let relays = match mullvad::fetch_relays().await {
        Ok(relays) => relays,
        Err(e) => {
            let _ = mullvad::delete_device(&token, &device.id).await;
            return Err(FfiError::from(e));
        }
    };
    let chosen = match relays.default_choice() {
        Some(chosen) => chosen,
        None => {
            let _ = mullvad::delete_device(&token, &device.id).await;
            return Err(FfiError::Network("no Mullvad relays are available".into()));
        }
    };
    let profile = mullvad::MullvadProfile {
        account_number: account,
        private_key,
        device_id: device.id,
        device_name: device.name,
        addresses: device.addresses,
        country_code: chosen.country_code,
        country_name: chosen.country_name,
        city_code: chosen.city_code,
        city_name: chosen.city_name,
    };
    Ok(profile_to_ffi(&profile))
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn mullvad_locations() -> Result<Vec<FfiMullvadCountry>, FfiError> {
    let relays = mullvad::fetch_relays().await.map_err(FfiError::from)?;
    Ok(relays
        .locations_tree()
        .into_iter()
        .map(|c| FfiMullvadCountry {
            code: c.code,
            name: c.name,
            cities: c
                .cities
                .into_iter()
                .map(|ci| FfiMullvadCity {
                    code: ci.code,
                    name: ci.name,
                })
                .collect(),
        })
        .collect())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn mullvad_sign_out(account_number: String, device_id: String) -> Result<(), FfiError> {
    let account = mullvad::normalize_account(&account_number).map_err(FfiError::from)?;
    let token = mullvad::fetch_token(&account)
        .await
        .map_err(FfiError::from)?;
    mullvad::delete_device(&token, &device_id)
        .await
        .map_err(FfiError::from)
}

#[uniffi::export]
pub fn mask_mullvad_account(account_number: String) -> String {
    mullvad::mask_account(&account_number)
}

#[uniffi::export]
pub fn media_cache_size(cache_dir: String) -> u64 {
    match require_prepared_cache_dir(&cache_dir) {
        Ok(dir) => crate::media_cache::size(&dir),
        Err(_) => 0,
    }
}

#[uniffi::export]
pub fn clear_media_cache(cache_dir: String) -> Result<(), FfiError> {
    let dir = require_prepared_cache_dir(&cache_dir)?;
    crate::media_cache::clear(&dir).map_err(FfiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("feline-ffi-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scoped_file_path_rejects_paths_outside_root() {
        let root = temp_dir("root").canonicalize().unwrap();
        let outside = temp_dir("outside");
        let outside_file = outside.join("file.jpg");

        let err = scoped_file_path(Some(&root), outside_file.to_str().unwrap()).unwrap_err();

        assert!(matches!(err, FfiError::InvalidArgument(_)));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn scoped_file_path_accepts_paths_inside_root() {
        let root = temp_dir("inside").canonicalize().unwrap();
        let dest = root.join("file.jpg");

        let scoped = scoped_file_path(Some(&root), dest.to_str().unwrap()).unwrap();

        assert_eq!(scoped, dest);
        let _ = fs::remove_dir_all(root);
    }
}
