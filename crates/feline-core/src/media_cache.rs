use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub const MAX_CACHE_BYTES: u64 = 1024 * 1024 * 1024;

const TMP_PREFIX: &str = ".tmp-";
const MARKER_FILENAME: &str = ".feline-media-cache";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn key(url: &str) -> String {
    hex::encode(Sha256::digest(url.as_bytes()))
}

fn is_temp(name: &str) -> bool {
    name.starts_with(TMP_PREFIX)
}

fn is_internal(name: &str) -> bool {
    is_temp(name) || name == MARKER_FILENAME
}

pub fn prepare_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).context("create media cache dir")?;
    let marker = dir.join(MARKER_FILENAME);
    if !marker.exists() {
        fs::File::create(marker).context("create media cache marker")?;
    }
    Ok(())
}

pub fn is_prepared_dir(dir: &Path) -> bool {
    dir.join(MARKER_FILENAME).is_file()
}

pub fn is_cacheable_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    !matches!(ext.as_str(), "webm" | "mp4" | "mov" | "m4v" | "swf")
}

pub fn read(dir: &Path, url: &str) -> Option<Vec<u8>> {
    let path = dir.join(key(url));
    let bytes = fs::read(&path).ok()?;
    if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(bytes)
}

pub fn write(dir: &Path, url: &str, bytes: &[u8], max: u64) -> Result<()> {
    prepare_dir(dir)?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{TMP_PREFIX}{}-{seq}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp).context("create cache temp file")?;
        file.write_all(bytes).context("write cache temp file")?;
        file.flush().context("flush cache temp file")?;
    }
    fs::rename(&tmp, dir.join(key(url))).context("commit cache file")?;
    enforce_cap(dir, max);
    Ok(())
}

pub fn size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let name = entry.file_name();
            if name.to_str().is_some_and(is_internal) {
                return None;
            }
            Some(meta.len())
        })
        .sum()
}

pub fn clear(dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.context("read media cache entry")?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_str().is_some_and(is_internal) {
            continue;
        }
        if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("remove media cache file `{}`", path.display()))?;
        }
    }
    Ok(())
}

fn enforce_cap(dir: &Path, max: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name();
        if name.to_str().is_some_and(is_internal) {
            continue;
        }
        let len = meta.len();
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        total = total.saturating_add(len);
        files.push((entry.path(), len, mtime));
    }
    if total <= max {
        return;
    }
    files.sort_by_key(|(_, _, mtime)| *mtime);
    for (path, len, _) in files {
        if total <= max {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "feline-media-cache-test-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn key_is_stable_and_distinct() {
        assert_eq!(key("https://x/a.jpg"), key("https://x/a.jpg"));
        assert_ne!(key("https://x/a.jpg"), key("https://x/b.jpg"));
        assert_eq!(key("https://x/a.jpg").len(), 64);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = temp_dir();
        let url = "https://static1.e621.net/a.jpg";
        write(&dir, url, b"hello", MAX_CACHE_BYTES).unwrap();
        assert_eq!(read(&dir, url).as_deref(), Some(b"hello".as_slice()));
        assert_eq!(read(&dir, "https://static1.e621.net/missing.jpg"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn videos_are_not_cacheable() {
        assert!(is_cacheable_url("https://x/a.jpg"));
        assert!(is_cacheable_url("https://x/a.png?foo=1"));
        assert!(!is_cacheable_url("https://x/a.webm"));
        assert!(!is_cacheable_url("https://x/a.mp4"));
    }

    #[test]
    fn enforce_cap_evicts_oldest_first() {
        let dir = temp_dir();
        write(&dir, "https://x/old.jpg", &[0u8; 100], MAX_CACHE_BYTES).unwrap();
        let old = dir.join(key("https://x/old.jpg"));
        let stale = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(stale)
            .unwrap();

        write(&dir, "https://x/new.jpg", &[0u8; 100], 150).unwrap();

        assert!(read(&dir, "https://x/old.jpg").is_none());
        assert!(read(&dir, "https://x/new.jpg").is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_preserves_cache_marker() {
        let dir = temp_dir();
        prepare_dir(&dir).unwrap();
        write(&dir, "https://x/a.jpg", b"hello", MAX_CACHE_BYTES).unwrap();

        clear(&dir).unwrap();

        assert!(is_prepared_dir(&dir));
        assert_eq!(size(&dir), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
