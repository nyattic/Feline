use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use feline_core::{MediaSkip, RatingFilter, Site};

use feline_core::util::{config_dir, default_download_dir, write_file_synced};

pub const DEFAULT_CONFIG_FILENAME: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagQuery {
    pub id: u64,
    pub tags: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub site: Site,
    pub download_dir: PathBuf,
    pub blacklist: Vec<String>,
    pub rating: RatingFilter,
    #[serde(default)]
    pub media_skip: MediaSkip,
    pub queries: Vec<TagQuery>,
    #[serde(default)]
    pub next_query_id: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            site: Site::default(),
            download_dir: default_download_dir(),
            blacklist: Vec::new(),
            rating: RatingFilter::all(),
            media_skip: MediaSkip::default(),
            queries: Vec::new(),
            next_query_id: 1,
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        config_dir().join(DEFAULT_CONFIG_FILENAME)
    }

    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
                Ok(mut cfg) => {
                    cfg.normalize();
                    cfg
                }
                Err(e) => {
                    preserve_invalid_file(path, "config");
                    tracing::warn!("failed to parse config, using default: {e}");
                    Config::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => {
                tracing::warn!("failed to read config, using default: {e}");
                Config::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let bytes = serde_json::to_vec_pretty(self).context("serialize config")?;
        let tmp = path.with_extension("json.tmp");
        write_file_synced(&tmp, &bytes).context("write tmp config")?;
        std::fs::rename(&tmp, path).context("rename tmp config")?;
        Ok(())
    }

    pub fn new_query(&mut self, tags: String) -> u64 {
        let id = self.next_query_id;
        self.next_query_id = self.next_query_id.saturating_add(1);
        self.queries.push(TagQuery {
            id,
            tags,
            enabled: true,
        });
        id
    }

    pub fn remove_query(&mut self, id: u64) {
        self.queries.retain(|q| q.id != id);
    }

    fn normalize(&mut self) {
        let next = self
            .queries
            .iter()
            .map(|q| q.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        if self.next_query_id < next {
            self.next_query_id = next;
        }
    }
}

fn preserve_invalid_file(path: &Path, label: &str) {
    let backup = invalid_backup_path(path);
    match std::fs::copy(path, &backup) {
        Ok(_) => tracing::warn!("preserved invalid {label} file at `{}`", backup.display()),
        Err(e) => tracing::warn!("failed to preserve invalid {label} file: {e}"),
    }
}

fn invalid_backup_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!("{name}.invalid-{stamp}"))
}

#[cfg(test)]
mod tests {
    use super::{Config, TagQuery};

    #[test]
    fn load_normalizes_next_query_id_for_legacy_config() {
        let raw = r#"{
            "site": "e621",
            "download_dir": "/tmp/feline",
            "blacklist": [],
            "rating": { "safe": true, "questionable": true, "explicit": true },
            "media_skip": {},
            "queries": [
                { "id": 3, "tags": "cat", "enabled": true },
                { "id": 7, "tags": "dog", "enabled": true }
            ]
        }"#;

        let mut cfg: Config = serde_json::from_str(raw).unwrap();
        cfg.normalize();

        assert_eq!(cfg.next_query_id, 8);
    }

    #[test]
    fn new_query_does_not_reuse_existing_ids_after_normalize() {
        let mut cfg = Config {
            queries: vec![TagQuery {
                id: 42,
                tags: "existing".into(),
                enabled: true,
            }],
            next_query_id: 0,
            ..Config::default()
        };

        cfg.normalize();
        assert_eq!(cfg.new_query("new".into()), 43);
    }
}
