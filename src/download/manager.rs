use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tokio::runtime::Handle;
use tokio::sync::{Notify, mpsc};

use super::dedup::Md5Index;
use super::worker::{DownloadError, download_post};
use crate::config::Config;
use crate::credentials::Credentials;
use crate::e621::Client;
use crate::state::StateStore;

pub const CONCURRENT_DOWNLOADS: usize = 4;

/// Events the manager emits back to the UI.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    JobStarted {
        job_id: u64,
        tags: String,
    },
    Discovering {
        job_id: u64,
        pages_scanned: u32,
        posts_queued: usize,
    },
    DiscoveryDone {
        job_id: u64,
        total_posts: usize,
        skipped_existing: usize,
        skipped_failed: usize,
    },
    Progress {
        job_id: u64,
        done: usize,
        failed: usize,
        total: usize,
        current: Option<String>,
        bytes_per_sec: u64,
    },
    PostFailed {
        job_id: u64,
        post_id: u64,
        error: String,
    },
    JobFinished {
        job_id: u64,
        done: usize,
        failed: usize,
        total: usize,
        duration_ms: u64,
    },
    JobCancelled {
        job_id: u64,
    },
    JobPaused {
        job_id: u64,
    },
    JobResumed {
        job_id: u64,
    },
    JobError {
        job_id: u64,
        error: String,
    },
}

#[derive(Debug)]
pub struct JobHandle {
    pub job_id: u64,
    control: Arc<JobControl>,
}

#[derive(Debug)]
pub(crate) struct JobControl {
    cancel: AtomicBool,
    paused: AtomicBool,
    wake: Notify,
}

impl JobHandle {
    pub fn cancel(&self) {
        self.control.cancel();
    }

    pub fn pause(&self) {
        self.control.pause();
    }

    pub fn resume(&self) {
        self.control.resume();
    }

    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }
}

impl JobControl {
    fn new() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            wake: Notify::new(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.wake.notify_waiters();
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        self.wake.notify_waiters();
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
        self.wake.notify_waiters();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub async fn wait_if_paused(&self) -> bool {
        loop {
            if self.is_cancelled() {
                return true;
            }
            if !self.is_paused() {
                return false;
            }
            let notified = self.wake.notified();
            if self.is_cancelled() {
                return true;
            }
            if !self.is_paused() {
                return false;
            }
            notified.await;
        }
    }
}

pub struct DownloadManager {
    rt: Handle,
    events: mpsc::UnboundedSender<DownloadEvent>,
    next_job_id: AtomicU64,
    state: StateStore,
}

impl DownloadManager {
    pub fn new(
        rt: Handle,
        state: StateStore,
    ) -> (Self, mpsc::UnboundedReceiver<DownloadEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                rt,
                events: tx,
                next_job_id: std::sync::atomic::AtomicU64::new(1),
                state,
            },
            rx,
        )
    }

    /// Spawns a new download job that walks every matching page and downloads
    /// posts not already on disk (MD5-deduplicated per tag folder).
    pub fn spawn_job(
        &self,
        tags: String,
        cfg: Config,
        creds: Option<Credentials>,
    ) -> anyhow::Result<JobHandle> {
        let job_id = self
            .next_job_id
            .fetch_add(1, Ordering::Relaxed);
        let control = Arc::new(JobControl::new());
        let events = self.events.clone();
        let state = self.state.clone();

        let tags_for_log = tags.clone();
        let control_for_task = control.clone();
        self.rt.spawn(async move {
            let _ = events.send(DownloadEvent::JobStarted {
                job_id,
                tags: tags_for_log.clone(),
            });
            if let Err(e) = run_job(
                job_id,
                tags,
                cfg,
                creds,
                control_for_task,
                events.clone(),
                state,
            )
            .await
            {
                let _ = events.send(DownloadEvent::JobError {
                    job_id,
                    error: format!("{e:#}"),
                });
            }
        });

        Ok(JobHandle {
            job_id,
            control,
        })
    }
}

/// Blocks while `paused` is set. Returns true if the job was cancelled (either
/// while waiting or already cancelled at entry) so the caller can bail out.
/// Emits JobPaused/JobResumed events exactly once per transition; no events if
/// we're not paused at entry.
async fn wait_while_paused(
    job_id: u64,
    control: &JobControl,
    events: &mpsc::UnboundedSender<DownloadEvent>,
) -> bool {
    if control.is_cancelled() {
        return true;
    }
    if !control.is_paused() {
        return false;
    }
    let _ = events.send(DownloadEvent::JobPaused { job_id });
    loop {
        if control.is_cancelled() {
            return true;
        }
        if !control.is_paused() {
            break;
        }
        // Register the waker *before* rechecking the flag to avoid a lost-wake
        // race (pause cleared between the check and await).
        let notified = control.wake.notified();
        if !control.is_paused() || control.is_cancelled() {
            break;
        }
        notified.await;
    }
    if control.is_cancelled() {
        return true;
    }
    let _ = events.send(DownloadEvent::JobResumed { job_id });
    false
}

enum NextDownload<T> {
    Completed(T),
    Exhausted,
    Cancelled,
}

type DownloadOutcome = (u64, String, u64, Result<std::path::PathBuf, DownloadError>);

struct ProgressState {
    done: usize,
    failed: usize,
    discovered_total: usize,
    bytes_in_window: u64,
    window_start: Instant,
}

fn handle_download_outcome(
    outcome: DownloadOutcome,
    job_id: u64,
    tags: &str,
    state: &StateStore,
    md5_index: &Md5Index,
    events: &mpsc::UnboundedSender<DownloadEvent>,
    progress: &mut ProgressState,
) {
    let (post_id, md5, size, result) = outcome;
    match result {
        Ok(path) => {
            progress.done += 1;
            progress.bytes_in_window = progress.bytes_in_window.saturating_add(size);
            md5_index.insert(&md5);
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string());
            let bps = compute_bps(&mut progress.window_start, &mut progress.bytes_in_window);
            let _ = events.send(DownloadEvent::Progress {
                job_id,
                done: progress.done,
                failed: progress.failed,
                total: progress.discovered_total,
                current: fname,
                bytes_per_sec: bps,
            });
        }
        Err(DownloadError::Cancelled) => {}
        Err(err) => {
            progress.failed += 1;
            let err_str = format!("{err}");
            let _ = events.send(DownloadEvent::PostFailed {
                job_id,
                post_id,
                error: err_str.clone(),
            });
            if err.is_permanent() {
                state.update(tags, |s| {
                    s.failed.insert(post_id);
                });
            }
            let bps = compute_bps(&mut progress.window_start, &mut progress.bytes_in_window);
            let _ = events.send(DownloadEvent::Progress {
                job_id,
                done: progress.done,
                failed: progress.failed,
                total: progress.discovered_total,
                current: None,
                bytes_per_sec: bps,
            });
            tracing::warn!("post {post_id} failed: {err_str}");
        }
    }
}

async fn next_download_or_control<Fut>(
    job_id: u64,
    futs: &mut FuturesUnordered<Fut>,
    control: &JobControl,
    events: &mpsc::UnboundedSender<DownloadEvent>,
) -> NextDownload<Fut::Output>
where
    Fut: Future + Unpin,
{
    loop {
        if control.is_cancelled() {
            return NextDownload::Cancelled;
        }
        if futs.is_empty() {
            return NextDownload::Exhausted;
        }

        tokio::select! {
            maybe = futs.next() => {
                return match maybe {
                    Some(output) => NextDownload::Completed(output),
                    None => NextDownload::Exhausted,
                };
            }
            _ = control.wake.notified() => {
                if wait_while_paused(job_id, control, events).await {
                    return NextDownload::Cancelled;
                }
            }
        }
    }
}

async fn run_job(
    job_id: u64,
    tags: String,
    cfg: Config,
    creds: Option<Credentials>,
    control: Arc<JobControl>,
    events: mpsc::UnboundedSender<DownloadEvent>,
    state: StateStore,
) -> anyhow::Result<()> {
    let client = Client::new(cfg.site, creds.clone())?;
    let download_root = cfg.download_dir.clone();
    tokio::fs::create_dir_all(&download_root).await.ok();

    let md5_index = tokio::task::spawn_blocking({
        let root = download_root.clone();
        let tags = tags.clone();
        move || Md5Index::scan(&root, &tags)
    })
    .await
    .unwrap_or_else(|_| Md5Index::empty());

    let existing_failed = state.get(&tags).failed.clone();

    let http = client.http().clone();
    let start = Instant::now();
    let mut skipped_existing: usize = 0;
    let mut skipped_failed: usize = 0;
    let mut before_id: Option<u64> = None;
    let mut pages_scanned: u32 = 0;
    let mut progress = ProgressState {
        done: 0,
        failed: 0,
        discovered_total: 0,
        bytes_in_window: 0,
        window_start: Instant::now(),
    };

    let mut futs: FuturesUnordered<BoxFuture<'static, DownloadOutcome>> = FuturesUnordered::new();

    loop {
        if wait_while_paused(job_id, &control, &events).await {
            let _ = events.send(DownloadEvent::JobCancelled { job_id });
            return Ok(());
        }

        let page = client
            .search_page(&tags, &cfg.blacklist, cfg.rating, before_id)
            .await?;

        pages_scanned += 1;
        if page.is_empty() {
            break;
        }

        let mut lowest_id_on_page = u64::MAX;
        for post in page {
            if wait_while_paused(job_id, &control, &events).await {
                let _ = events.send(DownloadEvent::JobCancelled { job_id });
                return Ok(());
            }

            if post.id < lowest_id_on_page {
                lowest_id_on_page = post.id;
            }
            if existing_failed.contains(&post.id) {
                skipped_failed += 1;
                continue;
            }
            if md5_index.contains(&post.file.md5) {
                skipped_existing += 1;
                continue;
            }
            if post.file.url.is_none() {
                skipped_failed += 1;
                continue;
            }

            while futs.len() >= CONCURRENT_DOWNLOADS {
                match next_download_or_control(job_id, &mut futs, &control, &events).await {
                    NextDownload::Completed(outcome) => handle_download_outcome(
                        outcome,
                        job_id,
                        &tags,
                        &state,
                        &md5_index,
                        &events,
                        &mut progress,
                    ),
                    NextDownload::Exhausted => break,
                    NextDownload::Cancelled => {
                        let _ = state.save();
                        let _ = events.send(DownloadEvent::JobCancelled { job_id });
                        return Ok(());
                    }
                }
            }

            progress.discovered_total += 1;
            let http = http.clone();
            let root = download_root.clone();
            let tags = tags.clone();
            let control = control.clone();
            futs.push(Box::pin(async move {
                let id = post.id;
                let md5 = post.file.md5.clone();
                let size = post.file.size;
                let res = download_post(&http, &post, &root, &tags, control).await;
                (id, md5, size, res)
            }));
        }

        let _ = events.send(DownloadEvent::Discovering {
            job_id,
            pages_scanned,
            posts_queued: progress.discovered_total,
        });

        if lowest_id_on_page == u64::MAX {
            break;
        }
        before_id = Some(lowest_id_on_page);
    }

    let _ = events.send(DownloadEvent::DiscoveryDone {
        job_id,
        total_posts: progress.discovered_total,
        skipped_existing,
        skipped_failed,
    });

    if progress.discovered_total == 0 {
        let _ = events.send(DownloadEvent::JobFinished {
            job_id,
            done: 0,
            failed: 0,
            total: 0,
            duration_ms: 0,
        });
        return Ok(());
    }

    while !futs.is_empty() {
        match next_download_or_control(job_id, &mut futs, &control, &events).await {
            NextDownload::Completed(outcome) => handle_download_outcome(
                outcome,
                job_id,
                &tags,
                &state,
                &md5_index,
                &events,
                &mut progress,
            ),
            NextDownload::Exhausted => break,
            NextDownload::Cancelled => {
                let _ = state.save();
                let _ = events.send(DownloadEvent::JobCancelled { job_id });
                return Ok(());
            }
        }

        if wait_while_paused(job_id, &control, &events).await {
            let _ = state.save();
            let _ = events.send(DownloadEvent::JobCancelled { job_id });
            return Ok(());
        }
    }

    state.update(&tags, |s| {
        s.last_run = Some(time::OffsetDateTime::now_utc().unix_timestamp());
    });
    if let Err(e) = state.save() {
        tracing::warn!("state save failed: {e}");
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let _ = events.send(DownloadEvent::JobFinished {
        job_id,
        done: progress.done,
        failed: progress.failed,
        total: progress.discovered_total,
        duration_ms,
    });
    Ok(())
}

fn compute_bps(window_start: &mut Instant, bytes_in_window: &mut u64) -> u64 {
    let elapsed = window_start.elapsed().as_secs_f64().max(0.001);
    if elapsed < 1.0 {
        return (*bytes_in_window as f64 / elapsed) as u64;
    }
    let bps = (*bytes_in_window as f64 / elapsed) as u64;
    *window_start = Instant::now();
    *bytes_in_window = 0;
    bps
}
