use crate::credentials::Credentials;
use crate::e621::client::Client;
use crate::e621::rate_limit::new_api_limiter;
use crate::vpn;
use crate::Site;
use std::sync::Arc;

#[derive(uniffi::Record)]
pub struct FfiCredentials {
    pub username: String,
    pub api_key: String,
}

impl From<FfiCredentials> for Credentials {
    fn from(c: FfiCredentials) -> Self {
        Credentials { username: c.username, api_key: c.api_key }
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

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<anyhow::Error> for FfiError {
    fn from(e: anyhow::Error) -> Self {
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
}

#[uniffi::export(async_runtime = "tokio")]
impl E621Core {
    #[uniffi::constructor]
    pub async fn new(
        site: FfiSite,
        credentials: Option<FfiCredentials>,
        proxy_url: Option<String>,
    ) -> Result<Arc<Self>, FfiError> {
        let creds = credentials.map(Into::into);
        let limiter = new_api_limiter();
        let client = Client::with_limiter(site.into(), creds, limiter, proxy_url)
            .await
            .map_err(FfiError::from)?;
        Ok(Arc::new(Self { inner: client }))
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
        Ok(FfiResponse { status: resp.status, body: resp.body })
    }

    pub async fn fetch_media(&self, url: String) -> Result<FfiResponse, FfiError> {
        let resp = self.inner.fetch_media(&url).await.map_err(FfiError::from)?;
        Ok(FfiResponse { status: resp.status, body: resp.body })
    }

    pub async fn download_to_file(&self, url: String, dest_path: String) -> Result<u16, FfiError> {
        self.inner
            .download_to_file(&url, &dest_path)
            .await
            .map_err(FfiError::from)
    }
}

#[derive(uniffi::Object)]
pub struct FelineVpn {
    inner: std::sync::Mutex<Option<vpn::VpnHandle>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl FelineVpn {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { inner: std::sync::Mutex::new(None) })
    }

    pub async fn enable(&self, config_text: String) -> Result<(), FfiError> {
        let cfg = vpn::parse(&config_text)
            .map_err(|e| FfiError::InvalidArgument(format!("{e:#}")))?;
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
        let previous = self.inner.lock().expect("vpn handle poisoned").take();
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.lock().expect("vpn handle poisoned").is_some()
    }

    pub fn proxy_url(&self) -> Option<String> {
        self.inner
            .lock()
            .expect("vpn handle poisoned")
            .as_ref()
            .and_then(|h| h.proxy_url())
    }
}

#[uniffi::export]
pub fn validate_wg_config(config_text: String) -> Result<(), FfiError> {
    vpn::parse(&config_text)
        .map(|_| ())
        .map_err(|e| FfiError::InvalidArgument(format!("{e:#}")))
}
