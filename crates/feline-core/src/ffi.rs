use crate::credentials::Credentials;
use crate::e621::client::Client;
use crate::e621::rate_limit::new_api_limiter;
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
    #[error("ECH unavailable: {0}")]
    EchUnavailable(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<anyhow::Error> for FfiError {
    fn from(e: anyhow::Error) -> Self {
        let s = format!("{e:#}");
        if s.contains("ECH") {
            FfiError::EchUnavailable(s)
        } else if s.contains("path must start with '/'") || s.contains("unsupported HTTP method") {
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
        fail_closed_ech: bool,
    ) -> Result<Arc<Self>, FfiError> {
        let creds = credentials.map(Into::into);
        let limiter = new_api_limiter();
        let client = Client::with_limiter(site.into(), creds, limiter, fail_closed_ech)
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
}
