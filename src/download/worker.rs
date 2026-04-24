use anyhow::anyhow;
use backon::{ExponentialBuilder, Retryable};
use futures::StreamExt;
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use super::manager::JobControl;
use crate::e621::Post;
use crate::util::sanitize_path_component;

pub const MAX_RETRIES: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("post has no file url (deleted or restricted)")]
    NoUrl,
    #[error("download cancelled")]
    Cancelled,
    #[error("md5 mismatch: expected {expected}, got {actual}")]
    Md5Mismatch { expected: String, actual: String },
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Resolves the final on-disk path for a post: `{root}/{tags}/{artist}__{md5}.{ext}`.
/// `tags` is the raw tag-query string the user entered; it becomes the folder
/// name (sanitized). Artist is embedded in the filename so the same image
/// downloaded under a different query still carries its attribution.
pub fn target_path(root: &Path, tags: &str, post: &Post) -> PathBuf {
    let folder = sanitize_path_component(tags);
    let artist = sanitize_path_component(post.primary_artist());
    let ext = sanitize_path_component(&post.file.ext);
    let md5 = post.file.md5.to_ascii_lowercase();
    root.join(folder).join(format!("{artist}__{md5}.{ext}"))
}

/// Downloads a single post with exponential backoff retry. On success returns
/// the final path. Any retriable network/IO error is retried up to MAX_RETRIES;
/// permanent errors (no url, md5 mismatch after all retries) bubble up.
pub async fn download_post(
    http: &reqwest::Client,
    post: &Post,
    download_root: &Path,
    tags: &str,
    control: Arc<JobControl>,
) -> Result<PathBuf, DownloadError> {
    let url = post
        .file
        .url
        .as_deref()
        .ok_or(DownloadError::NoUrl)?
        .to_string();
    let final_path = target_path(download_root, tags, post);

    if final_path.exists() {
        return Ok(final_path);
    }

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadError::Other(anyhow!("create parent dir: {e}")))?;
    }

    let expected_md5 = post.file.md5.to_ascii_lowercase();
    let tmp_path = final_path.with_extension(format!("{}.part", post.file.ext));

    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(30))
        .with_factor(2.0)
        .with_max_times(MAX_RETRIES)
        .with_jitter();

    let attempt = || async {
        stream_to_file_verified(http, &url, &tmp_path, &expected_md5, &control).await
    };

    let result = attempt
        .retry(backoff)
        .when(|e| match e {
            DownloadError::Md5Mismatch { .. } => true,
            DownloadError::Http { status, .. } => {
                // 4xx (except 408, 429) are not worth retrying.
                !(400..500).contains(status) || *status == 408 || *status == 429
            }
            DownloadError::Other(_) => true,
            DownloadError::NoUrl | DownloadError::Cancelled => false,
        })
        .notify(|e, dur| {
            tracing::warn!(?dur, "download retry for post {}: {}", post.id, e);
        })
        .await;

    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(err);
    }

    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| DownloadError::Other(anyhow!("rename tmp to final: {e}")))?;

    Ok(final_path)
}

async fn stream_to_file_verified(
    http: &reqwest::Client,
    url: &str,
    tmp_path: &Path,
    expected_md5: &str,
    control: &JobControl,
) -> Result<(), DownloadError> {
    if control.wait_if_paused().await {
        return Err(DownloadError::Cancelled);
    }

    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Other(anyhow!("get: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(DownloadError::Http {
            status: status.as_u16(),
            body: truncate(&body, 300),
        });
    }

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(tmp_path)
        .await
        .map_err(|e| DownloadError::Other(anyhow!("open tmp: {e}")))?;

    let mut hasher = Md5::new();
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        if control.wait_if_paused().await {
            return Err(DownloadError::Cancelled);
        }
        let chunk = chunk.map_err(|e| DownloadError::Other(anyhow!("chunk: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::Other(anyhow!("write: {e}")))?;
    }

    file.flush()
        .await
        .map_err(|e| DownloadError::Other(anyhow!("flush: {e}")))?;
    drop(file);

    let actual_md5 = hex::encode(hasher.finalize());
    if actual_md5 != expected_md5 {
        // Remove corrupted partial so next attempt starts clean.
        let _ = tokio::fs::remove_file(tmp_path).await;
        return Err(DownloadError::Md5Mismatch {
            expected: expected_md5.to_string(),
            actual: actual_md5,
        });
    }

    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

impl DownloadError {
    pub fn is_permanent(&self) -> bool {
        match self {
            DownloadError::NoUrl => true,
            DownloadError::Http { status, .. } => (400..500).contains(status) && *status != 408 && *status != 429,
            DownloadError::Cancelled | DownloadError::Md5Mismatch { .. } | DownloadError::Other(_) => false,
        }
    }
}
