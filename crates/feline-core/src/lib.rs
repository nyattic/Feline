#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub mod config;
pub mod credentials;
pub mod e621;
#[cfg(feature = "ffi")]
mod ffi;
pub mod media_cache;
pub mod util;
#[cfg(feature = "vpn")]
pub mod vpn;

pub use config::{MediaSkip, RatingFilter, Site};
pub use credentials::Credentials;
pub use e621::client::Client;
pub use e621::types::{Post, PostsResponse};
