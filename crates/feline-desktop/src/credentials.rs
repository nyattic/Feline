use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "feline";
const USER_KEY: &str = "default";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub api_key: String,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.username.is_empty() || self.api_key.is_empty()
    }
}

pub fn load() -> Result<Option<Credentials>> {
    let entry = Entry::new(SERVICE, USER_KEY).context("open keyring entry")?;
    match entry.get_password() {
        Ok(serialized) => {
            let creds: Credentials = serde_json::from_str(&serialized)
                .context("deserialize credentials from keyring")?;
            if creds.is_empty() {
                Ok(None)
            } else {
                Ok(Some(creds))
            }
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring read failed: {e}")),
    }
}

pub fn save(creds: &Credentials) -> Result<()> {
    let entry = Entry::new(SERVICE, USER_KEY).context("open keyring entry")?;
    let serialized = serde_json::to_string(creds).context("serialize credentials")?;
    entry
        .set_password(&serialized)
        .map_err(|e| anyhow::anyhow!("keyring write failed: {e}"))?;
    Ok(())
}

pub fn clear() -> Result<()> {
    let entry = Entry::new(SERVICE, USER_KEY).context("open keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring delete failed: {e}")),
    }
}
