uniffi::setup_scaffolding!();

pub mod config;
pub mod credentials;
pub mod e621;
pub mod media_cache;
pub mod util;
pub mod vpn;
mod ffi;

pub use credentials::Credentials;
pub use e621::client::Client;
pub use e621::types::{Post, PostsResponse};
pub use config::{MediaSkip, RatingFilter, Site};
