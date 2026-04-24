#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod app;
mod config;
mod credentials;
mod download;
mod e621;
mod logging;
mod state;
mod util;

slint::include_modules!();

use anyhow::Result;
use slint::ComponentHandle;
use std::sync::Arc;

fn main() -> Result<()> {
    let _log_guard = logging::init()?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("feline-rt")
        .build()?;
    let rt_handle = rt.handle().clone();

    std::thread::Builder::new()
        .name("tokio-driver".into())
        .spawn(move || {
            rt.block_on(async {
                let mut never = tokio::time::interval(std::time::Duration::from_secs(3600));
                loop {
                    never.tick().await;
                }
            });
        })?;

    let ui = AppWindow::new().map_err(|e| anyhow::anyhow!("slint new: {e}"))?;

    let (controller, events_rx) = app::Controller::new(rt_handle.clone());
    let controller = Arc::new(parking_lot::Mutex::new(controller));

    app::bind(&ui, controller.clone(), rt_handle, events_rx);

    ui.run().map_err(|e| anyhow::anyhow!("slint run: {e}"))?;

    controller.lock().shutdown();
    Ok(())
}
