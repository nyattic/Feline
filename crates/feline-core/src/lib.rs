uniffi::setup_scaffolding!();

pub mod config;
pub mod credentials;
pub mod e621;
mod ffi;
pub mod media_cache;
pub mod util;
pub mod vpn;

pub use config::{MediaSkip, RatingFilter, Site};
pub use credentials::Credentials;
pub use e621::client::Client;
pub use e621::types::{Post, PostsResponse};
