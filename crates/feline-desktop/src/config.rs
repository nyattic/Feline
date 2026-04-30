use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util::{config_dir, default_download_dir};

pub const DEFAULT_CONFIG_FILENAME: &str = "config.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    #[default]
    E621,
    E926,
}

impl Site {
    pub fn host(&self) -> &'static str {
        match self {
            Site::E621 => "e621.net",
            Site::E926 => "e926.net",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RatingFilter {
    pub safe: bool,
    pub questionable: bool,
    pub explicit: bool,
}

/// Optional `-type:...` filters injected into every search.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MediaSkip {
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub flash: bool,
    #[serde(default)]
    pub animation: bool,
}

impl MediaSkip {
    /// Returns the negated `-type:` tokens for the enabled skip flags.
    pub fn as_query_tokens(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.video {
            out.push("-type:webm");
        }
        if self.flash {
            out.push("-type:swf");
        }
        if self.animation {
            out.push("-type:gif");
        }
        out
    }
}

impl RatingFilter {
    pub fn all() -> Self {
        Self {
            safe: true,
            questionable: true,
            explicit: true,
        }
    }

    /// Returns e621 rating filter tokens to append to a search query.
    /// If all three are enabled or all three are disabled, no filter is applied.
    pub fn as_query_fragment(&self) -> Option<String> {
        let selected: Vec<&'static str> = [
            self.safe.then_some("s"),
            self.questionable.then_some("q"),
            self.explicit.then_some("e"),
        ]
        .into_iter()
        .flatten()
        .collect();

        if selected.is_empty() || selected.len() == 3 {
            return None;
        }

        if selected.len() == 1 {
            Some(format!("rating:{}", selected[0]))
        } else {
            Some(format!("rating:{}", selected.join(",")))
        }
    }
}

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
                Ok(cfg) => cfg,
                Err(e) => {
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
        std::fs::write(&tmp, &bytes).context("write tmp config")?;
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
}
