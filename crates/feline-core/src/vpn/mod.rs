pub mod config;
pub mod device;
pub mod dns;
pub mod mullvad;
pub mod runtime;
pub mod socks;

use std::time::Duration;

use anyhow::Result;

pub use config::{WgConfig, parse};

const HANDSHAKE_MAX_AGE: Duration = Duration::from_secs(180);
const CONNECT_GRACE: Duration = Duration::from_secs(20);

pub struct VpnHandle {
    engine: Option<runtime::EngineHandle>,
    socks: Option<socks::SocksHandle>,
}

impl VpnHandle {
    pub async fn start(cfg: WgConfig) -> Result<Self> {
        let engine = runtime::start(cfg).await?;
        let socks = socks::start(engine.cmd_sender()).await?;
        Ok(Self {
            engine: Some(engine),
            socks: Some(socks),
        })
    }

    pub fn proxy_url(&self) -> Option<String> {
        self.socks.as_ref().map(|s| s.proxy_url())
    }

    pub fn is_healthy(&self) -> bool {
        let (Some(engine), Some(_socks)) = (self.engine.as_ref(), self.socks.as_ref()) else {
            return false;
        };
        if engine.age() < CONNECT_GRACE {
            return true;
        }
        matches!(
            engine.handshake_age_ms(),
            Some(ms) if Duration::from_millis(ms) < HANDSHAKE_MAX_AGE
        )
    }

    pub async fn shutdown(mut self) {
        if let Some(socks) = self.socks.take() {
            socks.shutdown().await;
        }
        if let Some(engine) = self.engine.take() {
            engine.shutdown().await;
        }
    }
}
