pub mod dedup;
pub mod manager;
pub mod worker;

pub use manager::{DownloadEvent, DownloadManager, JobHandle};
