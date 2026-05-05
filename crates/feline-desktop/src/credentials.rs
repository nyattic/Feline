use anyhow::{Context, Result};
use keyring::Entry;

pub use feline_core::Credentials;

const SERVICE: &str = "feline";
const LEGACY_USER_KEY: &str = "default";
const INDEX_USER_KEY: &str = "active-username";

pub fn load() -> Result<Option<Credentials>> {
    if let Some(active) = read_index()? {
        let entry = Entry::new(SERVICE, &active).context("open keyring entry")?;
        return match entry.get_password() {
            Ok(serialized) => parse(&serialized),
            Err(keyring::Error::NoEntry) => {
                let _ = clear_index();
                Ok(None)
            }
            Err(e) => Err(anyhow::anyhow!("keyring read failed: {e}")),
        };
    }

    let legacy = Entry::new(SERVICE, LEGACY_USER_KEY).context("open keyring entry")?;
    match legacy.get_password() {
        Ok(serialized) => {
            let creds = parse(&serialized)?;
            if let Some(creds) = creds.as_ref() {
                save(creds)?;
                let _ = legacy.delete_credential();
            }
            Ok(creds)
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring read failed: {e}")),
    }
}

pub fn save(creds: &Credentials) -> Result<()> {
    if creds.username.trim().is_empty() {
        anyhow::bail!("cannot save credentials with empty username");
    }
    let serialized = serde_json::to_string(creds).context("serialize credentials")?;
    let entry = Entry::new(SERVICE, &creds.username).context("open keyring entry")?;
    entry
        .set_password(&serialized)
        .map_err(|e| anyhow::anyhow!("keyring write failed: {e}"))?;
    write_index(&creds.username)?;
    Ok(())
}

pub fn clear() -> Result<()> {
    if let Some(active) = read_index()? {
        let entry = Entry::new(SERVICE, &active).context("open keyring entry")?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(anyhow::anyhow!("keyring delete failed: {e}")),
        }
        clear_index()?;
    }
    Ok(())
}

fn parse(serialized: &str) -> Result<Option<Credentials>> {
    let creds: Credentials =
        serde_json::from_str(serialized).context("deserialize credentials from keyring")?;
    Ok(if creds.is_empty() { None } else { Some(creds) })
}

fn read_index() -> Result<Option<String>> {
    let entry = Entry::new(SERVICE, INDEX_USER_KEY).context("open keyring index")?;
    match entry.get_password() {
        Ok(name) if !name.trim().is_empty() => Ok(Some(name)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring index read failed: {e}")),
    }
}

fn write_index(name: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, INDEX_USER_KEY).context("open keyring index")?;
    entry
        .set_password(name)
        .map_err(|e| anyhow::anyhow!("keyring index write failed: {e}"))?;
    Ok(())
}

fn clear_index() -> Result<()> {
    let entry = Entry::new(SERVICE, INDEX_USER_KEY).context("open keyring index")?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring index delete failed: {e}")),
    }
}
