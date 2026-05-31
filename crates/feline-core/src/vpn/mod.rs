pub mod config;
pub mod device;
pub mod dns;
pub mod mullvad;
pub mod runtime;
pub mod socks;

use anyhow::Result;

pub use config::{WgConfig, parse};

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

    pub fn proxy_display_url(&self) -> Option<String> {
        self.socks.as_ref().map(|s| s.proxy_display_url())
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
