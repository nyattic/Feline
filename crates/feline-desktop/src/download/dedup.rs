use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use feline_core::util::sanitize_path_component;

#[derive(Clone, Default)]
pub struct Md5Index {
    inner: Arc<RwLock<HashMap<String, Vec<PathBuf>>>>,
}

impl Md5Index {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn scan(root: &Path, tags: &str) -> Self {
        let mut paths: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let folder = root.join(sanitize_path_component(tags));
        if !folder.exists() {
            return Self {
                inner: Arc::new(RwLock::new(paths)),
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
                && let Some(md5) = extract_md5(&stem)
            {
                paths
                    .entry(md5.to_string())
                    .or_default()
                    .push(entry.into_path());
            }
        }
        tracing::info!(
            "md5 index scanned: {} existing files at {}",
            paths.len(),
            folder.display()
        );
        Self {
            inner: Arc::new(RwLock::new(paths)),
        }
    }

    pub fn candidates(&self, md5: &str) -> Vec<PathBuf> {
        self.inner
            .read()
            .get(&md5.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn insert(&self, md5: &str, path: PathBuf) {
        self.inner
            .write()
            .entry(md5.to_ascii_lowercase())
            .or_default()
            .push(path);
    }
}

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
