use backon::{ExponentialBuilder, Retryable};
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use super::dedup::Md5Index;
use super::worker::{DownloadError, download_post};
use crate::config::Config;
use crate::credentials::Credentials;
use crate::state::StateStore;
use feline_core::e621::client::Client;
use feline_core::e621::rate_limit::{ApiLimiter, new_api_limiter};
use feline_core::e621::types::Post;

pub const CONCURRENT_DOWNLOADS: usize = 4;

const DISCOVERY_BUFFER: usize = 64;

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
        current: Option<String>,
        bytes_per_sec: u64,
    },
    PostFailed {
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
    limiter: Arc<ApiLimiter>,
}

impl DownloadManager {
    pub fn new(rt: Handle, state: StateStore) -> (Self, mpsc::UnboundedReceiver<DownloadEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                rt,
                events: tx,
                next_job_id: AtomicU64::new(1),
                state,
                limiter: new_api_limiter(),
            },
            rx,
        )
    }

    pub fn spawn_job(&self, tags: String, cfg: Config, creds: Option<Credentials>) -> JobHandle {
        let job_id = self.next_job_id.fetch_add(1, Ordering::Relaxed);
        let control = Arc::new(JobControl::new());
        let events = self.events.clone();
        let state = self.state.clone();
        let limiter = self.limiter.clone();

        let tags_for_log = tags.clone();
        let control_for_task = control.clone();
        self.rt.spawn(async move {
            let _ = events.send(DownloadEvent::JobStarted {
                job_id,
                tags: tags_for_log,
            });
            if let Err(e) = run_job(
                job_id,
                tags,
                cfg,
                creds,
                RunJobRuntime {
                    limiter,
                    control: control_for_task,
                    events: events.clone(),
                    state,
                },
            )
            .await
            {
                let _ = events.send(DownloadEvent::JobError {
                    job_id,
                    error: format!("{e:#}"),
                });
            }
        });

        JobHandle { job_id, control }
    }
}

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

type DownloadOutcome = (u64, String, u64, Result<std::path::PathBuf, DownloadError>);

struct ProgressState {
    done: usize,
    failed: usize,
    bytes_in_window: u64,
    window_start: Instant,
}

struct RunJobRuntime {
    limiter: Arc<ApiLimiter>,
    control: Arc<JobControl>,
    events: mpsc::UnboundedSender<DownloadEvent>,
    state: StateStore,
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
                current: fname,
                bytes_per_sec: bps,
            });
        }
        Err(DownloadError::Cancelled) => {}
        Err(err) => {
            progress.failed += 1;
            let err_str = format!("{err}");
            let _ = events.send(DownloadEvent::PostFailed {
                post_id,
                error: err_str.clone(),
            });
            if err.is_permanent() {
                mark_post_permanently_failed(state, tags, post_id);
            }
            let bps = compute_bps(&mut progress.window_start, &mut progress.bytes_in_window);
            let _ = events.send(DownloadEvent::Progress {
                job_id,
                done: progress.done,
                failed: progress.failed,
                current: None,
                bytes_per_sec: bps,
            });
            tracing::warn!("post {post_id} failed: {err_str}");
        }
    }
}

fn mark_post_permanently_failed(state: &StateStore, tags: &str, post_id: u64) {
    state.update(tags, |s| {
        s.failed.insert(post_id);
    });
    if let Err(e) = state.save() {
        tracing::warn!("state save failed after marking post {post_id} failed: {e}");
    }
}

async fn run_job(
    job_id: u64,
    tags: String,
    cfg: Config,
    creds: Option<Credentials>,
    runtime: RunJobRuntime,
) -> anyhow::Result<()> {
    let RunJobRuntime {
        limiter,
        control,
        events,
        state,
    } = runtime;
    let client = Client::with_limiter(cfg.site, creds.clone(), limiter, None).await?;
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

    let (post_tx, mut post_rx) = mpsc::channel::<Post>(DISCOVERY_BUFFER);
    let mut discovery: Option<JoinHandle<DiscoverySummary>> = Some(tokio::spawn(discover_stream(
        job_id,
        tags.clone(),
        cfg.clone(),
        client.clone(),
        state.clone(),
        existing_failed,
        md5_index.clone(),
        control.clone(),
        events.clone(),
        post_tx,
    )));

    let mut progress = ProgressState {
        done: 0,
        failed: 0,
        bytes_in_window: 0,
        window_start: Instant::now(),
    };
    let mut futs: FuturesUnordered<BoxFuture<'static, DownloadOutcome>> = FuturesUnordered::new();
    let mut dispatched: usize = 0;
    let mut channel_open = true;

    loop {
        if wait_while_paused(job_id, &control, &events).await {
            if let Some(handle) = discovery.take() {
                handle.abort();
            }
            let _ = state.save();
            let _ = events.send(DownloadEvent::JobCancelled { job_id });
            return Ok(());
        }

        if !channel_open && let Some(handle) = discovery.take() {
            let summary = handle.await.unwrap_or_default();
            if let Some(err) = summary.error {
                let _ = state.save();
                let _ = events.send(DownloadEvent::JobError { job_id, error: err });
                return Ok(());
            }
            if !summary.cancelled {
                let _ = events.send(DownloadEvent::DiscoveryDone {
                    job_id,
                    total_posts: summary.total,
                    skipped_existing: summary.skipped_existing,
                    skipped_failed: summary.skipped_failed,
                });
            }
        }

        while channel_open && futs.len() < CONCURRENT_DOWNLOADS {
            match post_rx.try_recv() {
                Ok(post) => {
                    dispatched += 1;
                    futs.push(spawn_download(post, &http, &download_root, &tags, &control));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => channel_open = false,
            }
        }

        if futs.is_empty() {
            if !channel_open {
                break;
            }
            tokio::select! {
                biased;
                _ = control.wake.notified() => {}
                maybe = post_rx.recv() => match maybe {
                    Some(post) => {
                        dispatched += 1;
                        futs.push(spawn_download(post, &http, &download_root, &tags, &control));
                    }
                    None => channel_open = false,
                },
            }
            continue;
        }

        tokio::select! {
            biased;
            _ = control.wake.notified() => {}
            outcome = futs.next() => {
                if let Some(outcome) = outcome {
                    handle_download_outcome(
                        outcome, job_id, &tags, &state, &md5_index, &events, &mut progress,
                    );
                }
            }
            maybe = post_rx.recv(), if channel_open && futs.len() < CONCURRENT_DOWNLOADS => {
                match maybe {
                    Some(post) => {
                        dispatched += 1;
                        futs.push(spawn_download(post, &http, &download_root, &tags, &control));
                    }
                    None => channel_open = false,
                }
            }
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
        total: dispatched,
        duration_ms,
    });
    Ok(())
}

#[derive(Default)]
struct DiscoverySummary {
    total: usize,
    skipped_existing: usize,
    skipped_failed: usize,
    cancelled: bool,
    error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn discover_stream(
    job_id: u64,
    tags: String,
    cfg: Config,
    client: Client,
    state: StateStore,
    existing_failed: std::collections::HashSet<u64>,
    md5_index: Md5Index,
    control: Arc<JobControl>,
    events: mpsc::UnboundedSender<DownloadEvent>,
    post_tx: mpsc::Sender<Post>,
) -> DiscoverySummary {
    let mut summary = DiscoverySummary::default();
    let mut before_id: Option<u64> = None;
    let mut pages_scanned: u32 = 0;

    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(15))
        .with_max_times(4)
        .with_jitter();

    loop {
        if control.wait_if_paused().await {
            summary.cancelled = true;
            return summary;
        }

        let page = match (|| async {
            client
                .search_page(&tags, &cfg.blacklist, cfg.rating, cfg.media_skip, before_id)
                .await
        })
        .retry(backoff)
        .notify(|err, dur| {
            tracing::warn!(?dur, "search retry for `{tags}`: {err:#}");
        })
        .await
        {
            Ok(page) => page,
            Err(err) => {
                summary.cancelled = true;
                summary.error = Some(format!("{err:#}"));
                return summary;
            }
        };

        pages_scanned += 1;
        if page.is_empty() {
            break;
        }

        let mut lowest_id_on_page = u64::MAX;
        for post in page {
            if post.id < lowest_id_on_page {
                lowest_id_on_page = post.id;
            }
            if existing_failed.contains(&post.id) {
                summary.skipped_failed += 1;
                continue;
            }
            if md5_index.contains(&post.file.md5) {
                summary.skipped_existing += 1;
                continue;
            }
            if post.file.url.is_none() {
                summary.skipped_failed += 1;
                mark_post_permanently_failed(&state, &tags, post.id);
                let _ = events.send(DownloadEvent::PostFailed {
                    post_id: post.id,
                    error: "post has no file url (deleted or restricted)".into(),
                });
                continue;
            }
            if post_tx.send(post).await.is_err() {
                summary.cancelled = true;
                return summary;
            }
            summary.total += 1;
        }

        let _ = events.send(DownloadEvent::Discovering {
            job_id,
            pages_scanned,
            posts_queued: summary.total,
        });

        if lowest_id_on_page == u64::MAX {
            break;
        }
        before_id = Some(lowest_id_on_page);
    }

    summary
}

fn spawn_download(
    post: Post,
    http: &wreq::Client,
    download_root: &std::path::Path,
    tags: &str,
    control: &Arc<JobControl>,
) -> BoxFuture<'static, DownloadOutcome> {
    let http = http.clone();
    let root = download_root.to_path_buf();
    let tags = tags.to_string();
    let control = control.clone();
    Box::pin(async move {
        let id = post.id;
        let md5 = post.file.md5.clone();
        let size = post.file.size;
        let res = download_post(&http, &post, &root, &tags, control).await;
        (id, md5, size, res)
    })
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
