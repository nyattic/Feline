#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app;
mod config;
mod credentials;
mod download;
mod logging;
mod state;
mod theme;
mod view;

use std::sync::Arc;

use anyhow::Result;
use iced::window;

use crate::app::App;
use crate::download::DownloadManager;
use crate::state::StateStore;

fn main() -> Result<()> {
    let _log_guard = logging::init()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("feline-rt")
        .build()?;
    let rt_handle = runtime.handle().clone();

    let state_store = StateStore::load(&StateStore::default_path());
    let (manager, events_rx) = DownloadManager::new(rt_handle, state_store);
    let manager = Arc::new(manager);
    let manager_for_boot = manager.clone();

    let mut events_rx = Some(events_rx);
    let mut manager_for_boot = Some(manager_for_boot);

    iced::application(App::title, App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .window(window::Settings {
            size: iced::Size::new(1100.0, 720.0),
            min_size: Some(iced::Size::new(800.0, 500.0)),
            ..Default::default()
        })
        .run_with(move || {
            App::boot(
                events_rx.take().expect("boot called twice"),
                manager_for_boot.take().expect("boot called twice"),
            )
        })
        .map_err(|e| anyhow::anyhow!("iced run: {e}"))?;

    drop(manager);
    drop(runtime);
    Ok(())
}
