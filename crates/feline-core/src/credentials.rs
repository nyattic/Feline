use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub api_key: String,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.username.trim().is_empty() || self.api_key.trim().is_empty()
    }
}
