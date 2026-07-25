use anyhow::anyhow;
use backon::{ExponentialBuilder, Retryable};
use futures::StreamExt;
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use super::manager::JobControl;
use feline_core::Site;
use feline_core::e621::types::Post;
use feline_core::util::{safe_truncate, sanitize_path_component};

pub const MAX_RETRIES: usize = 5;
const VERIFY_RETRIES: usize = 1;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("post has no file url (deleted or restricted)")]
    NoUrl,
    #[error("download cancelled")]
    Cancelled,
    #[error("md5 mismatch: expected {expected}, got {actual}")]
    Md5Mismatch { expected: String, actual: String },
    #[error("size mismatch: expected {expected} bytes, got {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error(
        "download exceeded expected size: expected {expected} bytes, got at least {actual} bytes"
    )]
    SizeExceeded { expected: u64, actual: u64 },
    #[error("invalid file url: {0}")]
    InvalidUrl(String),
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

struct TmpFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TmpFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn target_path(root: &Path, tags: &str, post: &Post) -> PathBuf {
    let folder = sanitize_path_component(tags);
    let artist = sanitize_path_component(post.primary_artist());
    let ext = sanitize_path_component(&post.file.ext);
    let md5 = post.file.md5.to_ascii_lowercase();
    root.join(folder).join(format!("{artist}__{md5}.{ext}"))
}

pub async fn download_post(
    http: &wreq::Client,
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
    validate_file_url(&url)?;
    let final_path = target_path(download_root, tags, post);
    let expected_md5 = post.file.md5.to_ascii_lowercase();

    if final_path.exists() {
        if existing_file_is_valid(&final_path, &expected_md5, post.file.size).await? {
            return Ok(final_path);
        }
        tracing::warn!(
            path = %final_path.display(),
            post_id = post.id,
            "existing file failed verification; redownloading"
        );
    }

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadError::Other(anyhow!("create parent dir: {e}")))?;
    }

    let tmp_path = final_path.with_extension(format!("{}.part", post.file.ext));
    let mut tmp_guard = TmpFileGuard::new(tmp_path.clone());

    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(30))
        .with_factor(2.0)
        .with_max_times(MAX_RETRIES)
        .with_jitter();

    let attempt = || async {
        stream_to_file_verified(
            http,
            &url,
            &tmp_path,
            &expected_md5,
            post.file.size,
            &control,
        )
        .await
    };

    let mut verify_remaining = VERIFY_RETRIES;
    let result = attempt
        .retry(backoff)
        .when(|e| match e {
            DownloadError::Md5Mismatch { .. }
            | DownloadError::SizeMismatch { .. }
            | DownloadError::SizeExceeded { .. } => {
                if verify_remaining > 0 {
                    verify_remaining -= 1;
                    true
                } else {
                    false
                }
            }
            DownloadError::Http { status, .. } => {
                !(400..500).contains(status) || *status == 408 || *status == 429
            }
            DownloadError::Other(_) => true,
            DownloadError::NoUrl | DownloadError::Cancelled | DownloadError::InvalidUrl(_) => false,
        })
        .notify(|e, dur| {
            tracing::warn!(?dur, "download retry for post {}: {}", post.id, e);
        })
        .await;

    result?;

    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| DownloadError::Other(anyhow!("rename tmp to final: {e}")))?;
    tmp_guard.disarm();

    Ok(final_path)
}

pub(crate) async fn existing_file_is_valid(
    path: &Path,
    expected_md5: &str,
    expected_size: u64,
) -> Result<bool, DownloadError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| DownloadError::Other(anyhow!("stat existing file: {e}")))?;
    if !meta.is_file() || meta.len() != expected_size {
        return Ok(false);
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| DownloadError::Other(anyhow!("open existing file: {e}")))?;
    let mut hasher = Md5::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| DownloadError::Other(anyhow!("read existing file: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()) == expected_md5)
}

async fn stream_to_file_verified(
    http: &wreq::Client,
    url: &str,
    tmp_path: &Path,
    expected_md5: &str,
    expected_size: u64,
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
            body: safe_truncate(&body, 300),
        });
    }

    if let Some(content_len) = resp.content_length()
        && content_len > expected_size
    {
        return Err(DownloadError::SizeExceeded {
            expected: expected_size,
            actual: content_len,
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
    let mut bytes_written = 0_u64;
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        if control.wait_if_paused().await {
            return Err(DownloadError::Cancelled);
        }
        let chunk = chunk.map_err(|e| DownloadError::Other(anyhow!("chunk: {e}")))?;
        bytes_written = bytes_written.saturating_add(chunk.len() as u64);
        if bytes_written > expected_size {
            let _ = tokio::fs::remove_file(tmp_path).await;
            return Err(DownloadError::SizeExceeded {
                expected: expected_size,
                actual: bytes_written,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::Other(anyhow!("write: {e}")))?;
    }

    file.flush()
        .await
        .map_err(|e| DownloadError::Other(anyhow!("flush: {e}")))?;
    file.sync_all()
        .await
        .map_err(|e| DownloadError::Other(anyhow!("sync tmp file: {e}")))?;
    drop(file);

    if bytes_written != expected_size {
        let _ = tokio::fs::remove_file(tmp_path).await;
        return Err(DownloadError::SizeMismatch {
            expected: expected_size,
            actual: bytes_written,
        });
    }

    let actual_md5 = hex::encode(hasher.finalize());
    if actual_md5 != expected_md5 {
        let _ = tokio::fs::remove_file(tmp_path).await;
        return Err(DownloadError::Md5Mismatch {
            expected: expected_md5.to_string(),
            actual: actual_md5,
        });
    }

    Ok(())
}

fn validate_file_url(raw: &str) -> Result<(), DownloadError> {
    let url = Url::parse(raw).map_err(|e| DownloadError::InvalidUrl(e.to_string()))?;
    if url.scheme() != "https" {
        return Err(DownloadError::InvalidUrl("file url must use https".into()));
    }
    let Some(host) = url.host_str() else {
        return Err(DownloadError::InvalidUrl("file url has no host".into()));
    };
    if Site::from_media_host(host).is_none() {
        return Err(DownloadError::InvalidUrl(format!(
            "host {host} is not allowed"
        )));
    }
    Ok(())
}

impl DownloadError {
    pub fn is_permanent(&self) -> bool {
        match self {
            DownloadError::NoUrl | DownloadError::InvalidUrl(_) => true,
            DownloadError::Http { status, .. } => {
                (400..500).contains(status) && *status != 408 && *status != 429
            }
            DownloadError::Cancelled
            | DownloadError::Md5Mismatch { .. }
            | DownloadError::SizeMismatch { .. }
            | DownloadError::SizeExceeded { .. }
            | DownloadError::Other(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TmpFileGuard, existing_file_is_valid, validate_file_url};
    use std::path::PathBuf;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("feline-worker-test-{name}-{}", std::process::id()))
    }

    #[test]
    fn armed_guard_removes_tmp_file_on_drop() {
        let path = temp_file("guard-armed");
        std::fs::write(&path, b"partial").unwrap();

        drop(TmpFileGuard::new(path.clone()));

        assert!(!path.exists(), "armed guard must remove the tmp file");
    }

    #[test]
    fn disarmed_guard_leaves_file_in_place() {
        let path = temp_file("guard-disarmed");
        std::fs::write(&path, b"final").unwrap();

        let mut guard = TmpFileGuard::new(path.clone());
        guard.disarm();
        drop(guard);

        assert!(path.exists(), "disarmed guard must not remove the file");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn dropping_future_mid_await_removes_tmp_file() {
        let path = temp_file("guard-cancelled-future");
        std::fs::write(&path, b"partial").unwrap();

        let mut fut = Box::pin({
            let path = path.clone();
            async move {
                let _guard = TmpFileGuard::new(path);
                std::future::pending::<()>().await;
            }
        });

        let poll = futures::poll!(fut.as_mut());
        assert!(
            poll.is_pending(),
            "future must park while holding the guard"
        );
        assert!(path.exists(), "tmp file survives while the future is alive");

        drop(fut);

        assert!(
            !path.exists(),
            "dropping the future mid-await must remove the tmp file"
        );
    }

    #[test]
    fn validates_expected_file_hosts() {
        assert!(validate_file_url("https://static1.e621.net/data/aa/bb/file.jpg").is_ok());
        assert!(validate_file_url("https://static2.e621.net/data/aa/bb/file.jpg").is_ok());
        assert!(validate_file_url("https://static1.e926.net/data/aa/bb/file.jpg").is_ok());
        assert!(validate_file_url("http://static1.e621.net/data/file.jpg").is_err());
        assert!(validate_file_url("https://example.com/file.jpg").is_err());
        assert!(validate_file_url("https://static1.e621.net.evil.com/file.jpg").is_err());
        assert!(validate_file_url("https://cdn.static1.e621.net/file.jpg").is_err());
    }

    #[tokio::test]
    async fn validates_existing_file_by_size_and_md5() {
        let path = temp_file("valid");
        tokio::fs::write(&path, b"hello").await.unwrap();

        assert!(
            existing_file_is_valid(&path, "5d41402abc4b2a76b9719d911017c592", 5)
                .await
                .unwrap()
        );
        assert!(
            !existing_file_is_valid(&path, "5d41402abc4b2a76b9719d911017c592", 6)
                .await
                .unwrap()
        );
        assert!(
            !existing_file_is_valid(&path, "00000000000000000000000000000000", 5)
                .await
                .unwrap()
        );

        let _ = tokio::fs::remove_file(path).await;
    }
}
