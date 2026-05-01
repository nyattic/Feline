uniffi::setup_scaffolding!();

pub mod config;
pub mod credentials;
pub mod e621;
pub mod util;
mod ffi;

pub use credentials::Credentials;
pub use e621::client::Client;
pub use e621::types::{Post, PostsResponse};
pub use config::{MediaSkip, RatingFilter, Site};
