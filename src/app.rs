use parking_lot::Mutex;
use slint::ComponentHandle;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::config::{Config, RatingFilter, Site};
use crate::credentials::Credentials;
use crate::download::{DownloadEvent, DownloadManager, JobHandle};
use crate::e621::Client;
use crate::state::StateStore;
use crate::{AppWindow, JobData, LogEntry, SettingsData, TagQueryData};

const LOG_LINE_CAP: usize = 2000;

pub struct Controller {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub cfg_dirty: bool,

    pub creds: Credentials,
    pub creds_loaded_from_store: bool,
    pub creds_store_error: Option<String>,
    pub creds_dirty: bool,

    pub state_store: StateStore,
    pub manager: Arc<DownloadManager>,

    pub jobs: HashMap<u64, JobState>,
    pub log_lines: VecDeque<LogLine>,
}

#[derive(Debug)]
pub struct JobState {
    pub tags: String,
    pub phase: JobPhase,
    /// Phase to restore on resume; populated only while paused.
    pub phase_before_pause: Option<JobPhase>,
    pub pages_scanned: u32,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub current_file: Option<String>,
    pub bytes_per_sec: u64,
    pub handle: Option<JobHandle>,
    pub finished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    Starting,
    Discovering,
    Downloading,
    Paused,
    Finished,
    Cancelled,
    Errored,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl Controller {
    pub fn new(rt: Handle) -> (Self, mpsc::UnboundedReceiver<DownloadEvent>) {
        let cfg_path = Config::default_path();
        let cfg = Config::load_or_default(&cfg_path);

        let (creds, creds_loaded, creds_err) = match crate::credentials::load() {
            Ok(Some(c)) => (c, true, None),
            Ok(None) => (Credentials::default(), false, None),
            Err(e) => (Credentials::default(), false, Some(format!("{e}"))),
        };

        let state_store = StateStore::load(&StateStore::default_path());
        let (manager, events_rx) = DownloadManager::new(rt, state_store.clone());

        let ctrl = Self {
            cfg,
            cfg_path,
            cfg_dirty: false,
            creds,
            creds_loaded_from_store: creds_loaded,
            creds_store_error: creds_err,
            creds_dirty: false,
            state_store,
            manager: Arc::new(manager),
            jobs: HashMap::new(),
            log_lines: VecDeque::with_capacity(LOG_LINE_CAP),
        };
        (ctrl, events_rx)
    }

    pub fn shutdown(&mut self) {
        self.save_config_if_dirty();
        for j in self.jobs.values() {
            if let Some(h) = &j.handle {
                h.cancel();
            }
        }
    }

    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.log_lines.len() >= LOG_LINE_CAP {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(LogLine { level, text: text.into() });
    }

    pub fn save_config_if_dirty(&mut self) {
        if !self.cfg_dirty {
            return;
        }
        match self.cfg.save(&self.cfg_path) {
            Ok(()) => self.cfg_dirty = false,
            Err(e) => self.push_log(LogLevel::Error, format!("config save failed: {e}")),
        }
    }

    fn persist_logged_in(&mut self) {
        match crate::credentials::save(&self.creds) {
            Ok(()) => {
                self.creds_dirty = false;
                self.creds_loaded_from_store = true;
                self.creds_store_error = None;
                self.push_log(LogLevel::Info, "logged in, credentials saved to OS keyring");
            }
            Err(e) => {
                self.creds_store_error = Some(format!("{e}"));
                self.push_log(LogLevel::Error, format!("credentials save failed: {e}"));
            }
        }
    }

    fn start_job(&mut self, tags: String) {
        if !self.creds_loaded_from_store || self.creds.is_empty() {
            self.push_log(LogLevel::Warn, "login required before downloading");
            return;
        }
        if self.jobs.values().any(|j| !j.finished && j.tags == tags) {
            self.push_log(
                LogLevel::Warn,
                format!("job already running for query: {tags}"),
            );
            return;
        }
        let cfg = self.cfg.clone();
        let creds = Some(self.creds.clone());
        match self.manager.spawn_job(tags.clone(), cfg, creds) {
            Ok(handle) => {
                let job_id = handle.job_id;
                self.jobs.insert(
                    job_id,
                    JobState {
                        tags,
                        phase: JobPhase::Starting,
                        phase_before_pause: None,
                        pages_scanned: 0,
                        total: 0,
                        done: 0,
                        failed: 0,
                        current_file: None,
                        bytes_per_sec: 0,
                        handle: Some(handle),
                        finished: false,
                    },
                );
            }
            Err(e) => self.push_log(LogLevel::Error, format!("spawn job: {e}")),
        }
    }

    fn handle_event(&mut self, ev: DownloadEvent) {
        match ev {
            DownloadEvent::JobStarted { job_id, tags } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Starting;
                }
                self.push_log(LogLevel::Info, format!("[{job_id}] started: {tags}"));
            }
            DownloadEvent::Discovering { job_id, pages_scanned, posts_queued } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Discovering;
                    j.pages_scanned = pages_scanned;
                    j.total = posts_queued;
                }
            }
            DownloadEvent::DiscoveryDone { job_id, total_posts, skipped_existing, skipped_failed } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Downloading;
                    j.total = total_posts;
                }
                self.push_log(
                    LogLevel::Info,
                    format!(
                        "[{job_id}] discovery done: {total_posts} to download, {skipped_existing} existing, {skipped_failed} previously failed/unavailable"
                    ),
                );
            }
            DownloadEvent::Progress { job_id, done, failed, total, current, bytes_per_sec } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.done = done;
                    j.failed = failed;
                    j.total = total;
                    j.current_file = current;
                    j.bytes_per_sec = bytes_per_sec;
                }
            }
            DownloadEvent::PostFailed { job_id, post_id, error } => {
                self.push_log(
                    LogLevel::Warn,
                    format!("[{job_id}] post {post_id} failed: {error}"),
                );
            }
            DownloadEvent::JobFinished { job_id, done, failed, total, duration_ms } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Finished;
                    j.finished = true;
                    j.done = done;
                    j.failed = failed;
                    j.total = total;
                    j.current_file = None;
                    j.handle = None;
                }
                self.push_log(
                    LogLevel::Info,
                    format!(
                        "[{job_id}] finished: {done}/{total} ok, {failed} failed, {:.1}s",
                        duration_ms as f64 / 1000.0
                    ),
                );
            }
            DownloadEvent::JobCancelled { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Cancelled;
                    j.phase_before_pause = None;
                    j.finished = true;
                    j.handle = None;
                }
                self.push_log(LogLevel::Warn, format!("[{job_id}] cancelled"));
            }
            DownloadEvent::JobPaused { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id)
                    && j.phase != JobPhase::Paused {
                        j.phase_before_pause = Some(j.phase);
                        j.phase = JobPhase::Paused;
                }
                self.push_log(LogLevel::Info, format!("[{job_id}] paused"));
            }
            DownloadEvent::JobResumed { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = j
                        .phase_before_pause
                        .take()
                        .unwrap_or(JobPhase::Downloading);
                    j.current_file = None;
                    j.bytes_per_sec = 0;
                }
                self.push_log(LogLevel::Info, format!("[{job_id}] resumed"));
            }
            DownloadEvent::JobError { job_id, error } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Errored;
                    j.finished = true;
                    j.handle = None;
                }
                self.push_log(LogLevel::Error, format!("[{job_id}] error: {error}"));
            }
        }
    }

    fn sync_settings_from_ui(&mut self, ui: &AppWindow) {
        let s = ui.get_settings();
        let new_username = s.username.to_string();
        let new_api_key = s.api_key.to_string();
        if new_username != self.creds.username || new_api_key != self.creds.api_key {
            self.creds.username = new_username;
            self.creds.api_key = new_api_key;
            self.creds_dirty = true;
        }
        let new_site = match s.site {
            0 => Site::E621,
            _ => Site::E926,
        };
        if new_site != self.cfg.site {
            self.cfg.site = new_site;
            self.cfg_dirty = true;
        }
        let new_dir = PathBuf::from(s.download_dir.to_string());
        if new_dir != self.cfg.download_dir {
            self.cfg.download_dir = new_dir;
            self.cfg_dirty = true;
        }
        let new_rating = RatingFilter {
            safe: s.rating_safe,
            questionable: s.rating_questionable,
            explicit: s.rating_explicit,
        };
        if (new_rating.safe, new_rating.questionable, new_rating.explicit)
            != (self.cfg.rating.safe, self.cfg.rating.questionable, self.cfg.rating.explicit)
        {
            self.cfg.rating = new_rating;
            self.cfg_dirty = true;
        }
        let new_blacklist: Vec<String> = s
            .blacklist
            .to_string()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if new_blacklist != self.cfg.blacklist {
            self.cfg.blacklist = new_blacklist;
            self.cfg_dirty = true;
        }
    }
}

// ---- UI push helpers (separate fns so they can borrow &Controller + &AppWindow) ----

fn to_tag_query_data(q: &crate::config::TagQuery, ctrl: &Controller) -> TagQueryData {
    let st = ctrl.state_store.get(&q.tags);
    TagQueryData {
        id: q.id as i32,
        tags: q.tags.clone().into(),
        failed_count: st.failed.len() as i32,
        running: ctrl.jobs.values().any(|j| !j.finished && j.tags == q.tags),
    }
}

fn to_job_data(id: u64, j: &JobState) -> JobData {
    let (phase_label, color_idx) = match j.phase {
        JobPhase::Starting => ("starting", 0),
        JobPhase::Discovering => ("discovering", 1),
        JobPhase::Downloading => ("downloading", 1),
        JobPhase::Paused => ("paused", 3),
        JobPhase::Finished => ("done", 2),
        JobPhase::Cancelled => ("cancelled", 3),
        JobPhase::Errored => ("error", 4),
    };
    let progress = if j.total > 0 {
        (j.done as f32 / j.total as f32).clamp(0.0, 1.0)
    } else if matches!(j.phase, JobPhase::Starting | JobPhase::Discovering) {
        0.0
    } else {
        1.0
    };
    JobData {
        id: id as i32,
        tags: j.tags.clone().into(),
        phase: phase_to_int(j.phase),
        phase_label: phase_label.into(),
        phase_color_idx: color_idx,
        stats_label: format_stats(j).into(),
        progress,
        current_file: j.current_file.clone().unwrap_or_default().into(),
        finished: j.finished,
        paused: j.phase == JobPhase::Paused,
    }
}

fn phase_to_int(p: JobPhase) -> i32 {
    match p {
        JobPhase::Starting => 0,
        JobPhase::Discovering => 1,
        JobPhase::Downloading => 2,
        JobPhase::Paused => 3,
        JobPhase::Finished => 4,
        JobPhase::Cancelled => 5,
        JobPhase::Errored => 6,
    }
}

fn format_stats(j: &JobState) -> String {
    let speed = format_bps(j.bytes_per_sec);
    match j.phase {
        JobPhase::Discovering => format!("{} pages · {} queued", j.pages_scanned, j.total),
        JobPhase::Downloading => format!("{}/{} · {} failed · {}", j.done, j.total, j.failed, speed),
        JobPhase::Paused => format!("{}/{} · {} failed · paused", j.done, j.total, j.failed),
        JobPhase::Finished | JobPhase::Cancelled => {
            format!("{}/{} · {} failed", j.done, j.total, j.failed)
        }
        _ => String::new(),
    }
}

fn format_bps(bps: u64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} MB/s", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{:.0} KB/s", bps as f64 / 1_000.0)
    } else if bps > 0 {
        format!("{bps} B/s")
    } else {
        "—".to_string()
    }
}

fn push_queries(ctrl: &Controller, ui: &AppWindow) {
    let items: Vec<TagQueryData> = ctrl
        .cfg
        .queries
        .iter()
        .map(|q| to_tag_query_data(q, ctrl))
        .collect();
    let model = Rc::new(slint::VecModel::from(items));
    ui.set_queries(slint::ModelRc::from(model));
}

fn push_jobs(ctrl: &Controller, ui: &AppWindow) {
    let mut sorted: Vec<(u64, &JobState)> = ctrl.jobs.iter().map(|(k, v)| (*k, v)).collect();
    sorted.sort_by_key(|(id, _)| *id);
    let items: Vec<JobData> = sorted.iter().map(|(id, j)| to_job_data(*id, j)).collect();
    let model = Rc::new(slint::VecModel::from(items));
    ui.set_jobs(slint::ModelRc::from(model));

    let active = ctrl.jobs.values().filter(|j| !j.finished).count() as i32;
    ui.set_active_jobs(active);
}

fn push_logs(ctrl: &Controller, ui: &AppWindow) {
    let items: Vec<LogEntry> = ctrl
        .log_lines
        .iter()
        .map(|l| LogEntry {
            level: match l.level {
                LogLevel::Info => 0,
                LogLevel::Warn => 1,
                LogLevel::Error => 2,
            },
            text: l.text.clone().into(),
        })
        .collect();
    let model = Rc::new(slint::VecModel::from(items));
    ui.set_logs(slint::ModelRc::from(model));
}

fn push_settings(ctrl: &Controller, ui: &AppWindow) {
    // Preserve the transient "checking" flag — it's driven by the login callback,
    // not by controller state, so we read the current value off the UI.
    let checking = ui.get_settings().creds_checking;
    ui.set_settings(SettingsData {
        username: ctrl.creds.username.clone().into(),
        api_key: ctrl.creds.api_key.clone().into(),
        creds_dirty: ctrl.creds_dirty,
        creds_loaded: ctrl.creds_loaded_from_store,
        creds_checking: checking,
        creds_error: ctrl.creds_store_error.clone().unwrap_or_default().into(),
        site: match ctrl.cfg.site {
            Site::E621 => 0,
            Site::E926 => 1,
        },
        download_dir: ctrl.cfg.download_dir.display().to_string().into(),
        rating_safe: ctrl.cfg.rating.safe,
        rating_questionable: ctrl.cfg.rating.questionable,
        rating_explicit: ctrl.cfg.rating.explicit,
        blacklist: ctrl.cfg.blacklist.join("\n").into(),
    });
    ui.set_logged_in(ctrl.creds_loaded_from_store);
}

fn push_all(ctrl: &Controller, ui: &AppWindow) {
    push_queries(ctrl, ui);
    push_jobs(ctrl, ui);
    push_logs(ctrl, ui);
    push_settings(ctrl, ui);
}

// ---- Binding ----

pub fn bind(
    ui: &AppWindow,
    controller: Arc<Mutex<Controller>>,
    rt: Handle,
    mut events_rx: mpsc::UnboundedReceiver<DownloadEvent>,
) {
    {
        let c = controller.lock();
        push_all(&c, ui);
    }

    // add-query
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_add_query(move |tags| {
            if let Some(ui) = ui_weak.upgrade() {
                let tags = tags.to_string().trim().to_string();
                if tags.is_empty() {
                    return;
                }
                let mut c = ctrl.lock();
                c.cfg.new_query(tags);
                c.cfg_dirty = true;
                c.save_config_if_dirty();
                push_queries(&c, &ui);
            }
        });
    }

    // remove-query
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_remove_query(move |id| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                c.cfg.remove_query(id as u64);
                c.cfg_dirty = true;
                c.save_config_if_dirty();
                push_queries(&c, &ui);
            }
        });
    }

    // start-job
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_start_job(move |tags| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                c.start_job(tags.to_string());
                push_queries(&c, &ui);
                push_jobs(&c, &ui);
                push_logs(&c, &ui);
            }
        });
    }

    // toggle-pause-job
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_toggle_pause_job(move |id| {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                if let Some(j) = c.jobs.get_mut(&(id as u64))
                    && let Some(h) = &j.handle {
                        if h.is_paused() {
                            h.resume();
                            j.phase = j.phase_before_pause.take().unwrap_or(JobPhase::Downloading);
                            j.current_file = None;
                            j.bytes_per_sec = 0;
                        } else if j.phase != JobPhase::Paused {
                            h.pause();
                            j.phase_before_pause = Some(j.phase);
                            j.phase = JobPhase::Paused;
                        }
                        push_jobs(&c, &ui);
                    }
            }
        });
    }

    // login — verify credentials with e621, then persist on success
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        let rt_for_login = rt.clone();
        ui.on_login(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (creds, site) = {
                let mut c = ctrl.lock();
                c.sync_settings_from_ui(&ui);
                (c.creds.clone(), c.cfg.site)
            };
            if creds.is_empty() {
                return;
            }

            // Flip the UI into "checking" state.
            let mut s = ui.get_settings();
            s.creds_checking = true;
            s.creds_error = "".into();
            ui.set_settings(s);

            let ui_weak_inner = ui.as_weak();
            let ctrl_inner = ctrl.clone();
            rt_for_login.spawn(async move {
                let result: Result<(), String> = async {
                    let client = Client::new(site, Some(creds.clone()))
                        .map_err(|e| format!("{e}"))?;
                    client.verify_login().await.map_err(|e| format!("{e:#}"))?;
                    Ok(())
                }
                .await;

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui_weak_inner.upgrade() else {
                        return;
                    };
                    let mut c = ctrl_inner.lock();
                    match result {
                        Ok(()) => {
                            c.persist_logged_in();
                        }
                        Err(err) => {
                            c.creds_loaded_from_store = false;
                            c.creds_store_error = Some(err.clone());
                            c.push_log(LogLevel::Error, format!("login failed: {err}"));
                        }
                    }
                    // Clear checking flag, then push full settings.
                    let mut s = ui.get_settings();
                    s.creds_checking = false;
                    ui.set_settings(s);
                    push_settings(&c, &ui);
                    push_queries(&c, &ui);
                    push_logs(&c, &ui);
                });
            });
        });
    }

    // logout — drop credentials and wipe keyring
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_logout(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                c.creds = Credentials::default();
                c.creds_dirty = false;
                c.creds_loaded_from_store = false;
                c.creds_store_error = None;
                if let Err(e) = crate::credentials::clear() {
                    c.push_log(LogLevel::Warn, format!("logout: {e}"));
                } else {
                    c.push_log(LogLevel::Info, "logged out");
                }
                push_settings(&c, &ui);
                push_queries(&c, &ui);
                push_logs(&c, &ui);
            }
        });
    }

    // pick-folder
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_pick_folder(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let start = ctrl.lock().cfg.download_dir.clone();
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose download folder")
                    .set_directory(&start)
                    .pick_folder()
                {
                    let mut c = ctrl.lock();
                    c.cfg.download_dir = path;
                    c.cfg_dirty = true;
                    c.save_config_if_dirty();
                    push_settings(&c, &ui);
                }
            }
        });
    }

    // settings-changed: auto-commit config-side fields
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_settings_changed(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                c.sync_settings_from_ui(&ui);
                c.save_config_if_dirty();
            }
        });
    }

    // clear-logs
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        ui.on_clear_logs(move || {
            if let Some(ui) = ui_weak.upgrade() {
                let mut c = ctrl.lock();
                c.log_lines.clear();
                push_logs(&c, &ui);
            }
        });
    }

    // Tokio → Slint event forwarder
    {
        let ui_weak = ui.as_weak();
        let ctrl = controller.clone();
        rt.spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                let ui_weak = ui_weak.clone();
                let ctrl = ctrl.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_weak.upgrade() {
                        let mut c = ctrl.lock();
                        c.handle_event(ev);
                        push_queries(&c, &ui);
                        push_jobs(&c, &ui);
                        push_logs(&c, &ui);
                    }
                });
            }
        });
    }
}
