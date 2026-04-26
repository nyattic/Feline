use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::util::state_dir;

pub const DEFAULT_STATE_FILENAME: &str = "state.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryState {
    /// Post ids permanently skipped (retry budget exhausted).
    #[serde(default)]
    pub failed: HashSet<u64>,
    /// Unix seconds of last successful run.
    #[serde(default)]
    pub last_run: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateFile {
    /// Keyed by raw tag string (exactly what the user entered).
    #[serde(default)]
    pub queries: BTreeMap<String, QueryState>,
}

#[derive(Clone)]
pub struct StateStore {
    inner: Arc<Mutex<StateFile>>,
    path: PathBuf,
}

impl StateStore {
    pub fn default_path() -> PathBuf {
        state_dir().join(DEFAULT_STATE_FILENAME)
    }

    pub fn load(path: &Path) -> Self {
        let data = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<StateFile>(&bytes).unwrap_or_else(|e| {
                tracing::warn!("failed to parse state, starting fresh: {e}");
                StateFile::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StateFile::default(),
            Err(e) => {
                tracing::warn!("failed to read state, starting fresh: {e}");
                StateFile::default()
            }
        };
        Self {
            inner: Arc::new(Mutex::new(data)),
            path: path.to_path_buf(),
        }
    }

    pub fn get(&self, tags: &str) -> QueryState {
        self.inner
            .lock()
            .queries
            .get(tags)
            .cloned()
            .unwrap_or_default()
    }

    pub fn update<F>(&self, tags: &str, f: F)
    where
        F: FnOnce(&mut QueryState),
    {
        let mut guard = self.inner.lock();
        let entry = guard.queries.entry(tags.to_string()).or_default();
        f(entry);
    }

    pub fn save(&self) -> Result<()> {
        let bytes = {
            let guard = self.inner.lock();
            serde_json::to_vec_pretty(&*guard).context("serialize state")?
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).context("write tmp state")?;
        std::fs::rename(&tmp, &self.path).context("rename tmp state")?;
        Ok(())
    }
}
