use parking_lot::RwLock;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use walkdir::WalkDir;

use crate::util::sanitize_path_component;

/// In-memory set of MD5 hashes already present in a single tag folder.
/// Scoped per-tag (not global), so the same image can legitimately be
/// downloaded into multiple tag folders — each folder is a self-contained
/// snapshot of its query.
#[derive(Clone, Default)]
pub struct Md5Index {
    inner: Arc<RwLock<HashSet<String>>>,
}

impl Md5Index {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Walks `{root}/{sanitized tags}` once and collects MD5 hashes from
    /// filenames of the form `{artist}__{md5}.{ext}` (and the legacy
    /// `{md5}.{ext}` shape, for files left over from the old layout).
    pub fn scan(root: &Path, tags: &str) -> Self {
        let mut set = HashSet::new();
        let folder = root.join(sanitize_path_component(tags));
        if !folder.exists() {
            return Self {
                inner: Arc::new(RwLock::new(set)),
            };
        }
        for entry in WalkDir::new(&folder)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let stem = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            if let Some(stem) = stem
                && let Some(md5) = extract_md5(&stem) {
                    set.insert(md5.to_string());
                }
            }
        tracing::info!(
            "md5 index scanned: {} existing files at {}",
            set.len(),
            folder.display()
        );
        Self {
            inner: Arc::new(RwLock::new(set)),
        }
    }

    pub fn contains(&self, md5: &str) -> bool {
        self.inner.read().contains(&md5.to_ascii_lowercase())
    }

    pub fn insert(&self, md5: &str) {
        self.inner.write().insert(md5.to_ascii_lowercase());
    }
}

/// Extracts the md5 hex component from a filename stem. Supports both the
/// current `{artist}__{md5}` shape and a bare `{md5}` stem.
fn extract_md5(stem: &str) -> Option<&str> {
    let candidate = stem.rsplit("__").next()?;
    if is_md5_hex(candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn is_md5_hex(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
