use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use iced::widget::{container, row, text_editor};
use iced::{Element, Length, Subscription, Task};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::config::{Config, Site, TagQuery};
use crate::credentials::Credentials;
use crate::download::{DownloadEvent, DownloadManager, JobHandle};
use crate::state::StateStore;
use crate::theme;
use crate::view;
use feline_core::e621::Client;

const LOG_LINE_CAP: usize = 2000;
const FINISHED_JOB_CAP: usize = 50;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const PHASE_TICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Queue,
    Settings,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteOption {
    E621,
    E926,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadLimitOption {
    Posts500,
    Posts2000,
    Posts5000,
    Posts10000,
    Unlimited,
}

impl DownloadLimitOption {
    fn from_limit(limit: u32) -> Self {
        match limit {
            500 => Self::Posts500,
            2_000 => Self::Posts2000,
            5_000 => Self::Posts5000,
            10_000 => Self::Posts10000,
            _ => Self::Unlimited,
        }
    }

    fn into_limit(self) -> u32 {
        match self {
            Self::Posts500 => 500,
            Self::Posts2000 => 2_000,
            Self::Posts5000 => 5_000,
            Self::Posts10000 => 10_000,
            Self::Unlimited => 0,
        }
    }
}

impl std::fmt::Display for DownloadLimitOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Posts500 => f.write_str("500 posts"),
            Self::Posts2000 => f.write_str("2,000 posts"),
            Self::Posts5000 => f.write_str("5,000 posts"),
            Self::Posts10000 => f.write_str("10,000 posts"),
            Self::Unlimited => f.write_str("Unlimited"),
        }
    }
}

impl SiteOption {
    fn from_site(s: Site) -> Self {
        match s {
            Site::E621 => Self::E621,
            Site::E926 => Self::E926,
        }
    }

    fn into_site(self) -> Site {
        match self {
            Self::E621 => Site::E621,
            Self::E926 => Site::E926,
        }
    }
}

impl std::fmt::Display for SiteOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::E621 => f.write_str("e621.net (NSFW)"),
            Self::E926 => f.write_str("e926.net (SFW)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub timestamp: String,
    pub text: String,
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

pub struct JobState {
    pub tags: String,
    pub phase: JobPhase,
    pub phase_before_pause: Option<JobPhase>,
    pub pages_scanned: u32,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub current_file: Option<String>,
    pub bytes_per_sec: u64,
    pub handle: Option<JobHandle>,
    pub discovering: bool,
    pub finished: bool,
}

#[derive(Debug, Clone)]
pub struct QueryView {
    pub id: u64,
    pub tags: String,
    pub failed_count: usize,
    pub running: bool,
    pub queued: bool,
    pub last_run: String,
}

#[derive(Debug, Clone)]
pub struct JobView {
    pub id: u64,
    pub tags: String,
    pub phase_color_idx: u8,
    pub phase_label: String,
    pub phase_dots: String,
    pub stats_label: String,
    pub progress: f32,
    pub current_file: String,
    pub finished: bool,
    pub paused: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsForm {
    pub username: String,
    pub api_key: String,
    pub api_key_saved: bool,
    pub creds_loaded: bool,
    pub creds_checking: bool,
    pub creds_error: String,
    pub creds_dirty: bool,
    pub config_save_error: String,
    pub site: SiteOption,
    pub download_dir: String,
    pub rating_safe: bool,
    pub rating_questionable: bool,
    pub rating_explicit: bool,
    pub skip_video: bool,
    pub skip_flash: bool,
    pub skip_animation: bool,
    pub download_limit: DownloadLimitOption,
    pub blacklist: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    TabSelected(Tab),
    NewQueryChanged(String),
    StartJob(String),
    RemoveQuery(u64),
    UndoRemoveQuery,
    CancelQuery(u64),
    ClearQueryFailures(u64),
    TogglePauseJob(u64),
    ClearFinishedJobs,
    OpenDownloadFolder,
    DownloadFolderOpened(Result<(), String>),

    UsernameChanged(String),
    ApiKeyChanged(String),
    SiteChanged(SiteOption),
    DownloadDirChanged(String),
    PickFolder,
    FolderPicked(Option<PathBuf>),
    RatingSafe(bool),
    RatingQuestionable(bool),
    RatingExplicit(bool),
    SkipVideo(bool),
    SkipFlash(bool),
    SkipAnimation(bool),
    DownloadLimitChanged(DownloadLimitOption),
    BlacklistEdited(text_editor::Action),
    ConfigSaveTick(u64),

    Login,
    LoginCompleted {
        token: u64,
        creds: Credentials,
        result: Result<(), String>,
    },
    Logout,

    ClearLogs,

    DownloadEvent(DownloadEvent),
    PhaseTick,
}

pub struct App {
    cfg: Config,
    cfg_path: PathBuf,
    save_token: u64,

    creds: Credentials,
    active_creds: Option<Credentials>,
    creds_checking: bool,
    login_token: u64,

    state_store: StateStore,
    manager: Arc<DownloadManager>,

    jobs: HashMap<u64, JobState>,
    pending_tags: VecDeque<String>,
    log_lines: VecDeque<LogLine>,
    removed_query: Option<(usize, TagQuery)>,

    active_tab: Tab,
    new_query_buf: String,
    settings_form: SettingsForm,
    blacklist_content: text_editor::Content,
    phase_tick: u8,
}

impl App {
    pub fn boot(
        events_rx: UnboundedReceiver<DownloadEvent>,
        manager: Arc<DownloadManager>,
        state_store: StateStore,
    ) -> (Self, Task<Message>) {
        let cfg_path = Config::default_path();
        let cfg = Config::load_or_default(&cfg_path);

        let (creds, creds_loaded, creds_err) = match crate::credentials::load() {
            Ok(Some(c)) => (c, true, None),
            Ok(None) => (Credentials::default(), false, None),
            Err(e) => (Credentials::default(), false, Some(format!("{e}"))),
        };

        let settings_form = build_settings_form(&cfg, &creds, creds_loaded, creds_err.as_deref());
        let blacklist_content = text_editor::Content::with_text(&cfg.blacklist.join("\n"));
        let active_creds = creds_loaded.then(|| creds.clone());

        let app = Self {
            cfg,
            cfg_path,
            save_token: 0,
            creds,
            active_creds,
            creds_checking: false,
            login_token: 0,
            state_store,
            manager,
            jobs: HashMap::new(),
            pending_tags: VecDeque::new(),
            log_lines: VecDeque::with_capacity(LOG_LINE_CAP),
            removed_query: None,
            active_tab: Tab::Queue,
            new_query_buf: String::new(),
            settings_form,
            blacklist_content,
            phase_tick: 0,
        };

        let stream = UnboundedReceiverStream::new(events_rx).map(Message::DownloadEvent);
        let event_task = Task::stream(stream);
        (app, event_task)
    }

    pub fn title(&self) -> String {
        "Feline".to_string()
    }

    pub fn theme(&self) -> iced::Theme {
        theme::build()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let any_active_phase = self.jobs.values().any(|j| {
            !j.finished
                && matches!(
                    j.phase,
                    JobPhase::Starting | JobPhase::Discovering | JobPhase::Downloading
                )
        });
        let phase = if any_active_phase {
            iced::time::every(PHASE_TICK).map(|_| Message::PhaseTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([phase, iced::event::listen_with(keyboard_shortcut)])
    }

    pub fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::TabSelected(tab) => {
                self.active_tab = tab;
                Task::none()
            }
            Message::NewQueryChanged(s) => {
                self.new_query_buf = s;
                Task::none()
            }
            Message::StartJob(tags) => {
                self.start_job(tags);
                self.new_query_buf.clear();
                Task::none()
            }
            Message::RemoveQuery(id) => {
                let removed = self
                    .cfg
                    .queries
                    .iter()
                    .position(|q| q.id == id)
                    .map(|index| (index, self.cfg.queries[index].clone()));
                self.cfg.remove_query(id);
                if let Some((index, query)) = removed {
                    self.pending_tags.retain(|tags| tags != &query.tags);
                    self.removed_query = Some((index, query));
                }
                self.persist_config_now();
                Task::none()
            }
            Message::UndoRemoveQuery => {
                let Some((index, query)) = self.removed_query.take() else {
                    return Task::none();
                };
                if !self.cfg.queries.iter().any(|item| item.tags == query.tags) {
                    let index = index.min(self.cfg.queries.len());
                    self.cfg.queries.insert(index, query);
                    self.persist_config_now();
                }
                Task::none()
            }
            Message::CancelQuery(id) => {
                let Some(tags) = self
                    .cfg
                    .queries
                    .iter()
                    .find(|q| q.id == id)
                    .map(|q| q.tags.clone())
                else {
                    return Task::none();
                };
                let was_pending = self.pending_tags.iter().any(|t| t == &tags);
                self.pending_tags.retain(|t| t != &tags);
                let active_id = self
                    .jobs
                    .iter()
                    .find(|(_, j)| !j.finished && j.tags == tags)
                    .map(|(k, _)| *k);
                if let Some(job_id) = active_id {
                    if let Some(j) = self.jobs.get(&job_id)
                        && let Some(h) = j.handle.as_ref()
                    {
                        h.cancel();
                    }
                    self.push_log(LogLevel::Info, format!("cancel requested: {tags}"));
                } else if was_pending {
                    self.push_log(LogLevel::Info, format!("removed from queue: {tags}"));
                }
                Task::none()
            }
            Message::ClearQueryFailures(id) => {
                let Some(tags) = self
                    .cfg
                    .queries
                    .iter()
                    .find(|q| q.id == id)
                    .map(|q| q.tags.clone())
                else {
                    return Task::none();
                };
                self.state_store.clear_failed(&tags);
                if let Err(e) = self.state_store.save() {
                    self.push_log(LogLevel::Error, format!("state save failed: {e}"));
                } else {
                    self.push_log(LogLevel::Info, format!("reset skipped posts: {tags}"));
                }
                Task::none()
            }
            Message::TogglePauseJob(id) => {
                if let Some(j) = self.jobs.get_mut(&id)
                    && let Some(h) = &j.handle
                {
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
                }
                Task::none()
            }
            Message::ClearFinishedJobs => {
                self.jobs.retain(|_, job| !job.finished);
                Task::none()
            }
            Message::OpenDownloadFolder => {
                let path = self.cfg.download_dir.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || open_download_folder(&path))
                            .await
                            .map_err(|err| format!("folder opener failed: {err}"))?
                    },
                    Message::DownloadFolderOpened,
                )
            }
            Message::DownloadFolderOpened(result) => {
                match result {
                    Ok(()) => self.push_log(LogLevel::Info, "opened download folder"),
                    Err(err) => {
                        self.push_log(LogLevel::Error, format!("open download folder: {err}"))
                    }
                }
                Task::none()
            }

            Message::UsernameChanged(v) => {
                self.settings_form.username = v.clone();
                self.settings_form.api_key_saved = false;
                self.creds.username = v;
                self.invalidate_login_attempt();
                self.sync_credentials_dirty();
                Task::none()
            }
            Message::ApiKeyChanged(v) => {
                self.settings_form.api_key = v.clone();
                self.settings_form.api_key_saved = false;
                self.creds.api_key = v;
                self.invalidate_login_attempt();
                self.sync_credentials_dirty();
                Task::none()
            }
            Message::SiteChanged(site) => {
                self.settings_form.site = site;
                self.cfg.site = site.into_site();
                self.schedule_save()
            }
            Message::DownloadDirChanged(dir) => {
                self.settings_form.download_dir = dir.clone();
                self.cfg.download_dir = PathBuf::from(dir);
                self.schedule_save()
            }
            Message::PickFolder => {
                let start = self.cfg.download_dir.clone();
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose download folder")
                            .set_directory(start)
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::FolderPicked,
                )
            }
            Message::FolderPicked(Some(path)) => {
                self.cfg.download_dir = path.clone();
                self.settings_form.download_dir = path.display().to_string();
                self.persist_config_now();
                Task::none()
            }
            Message::FolderPicked(None) => Task::none(),
            Message::RatingSafe(v) => {
                self.settings_form.rating_safe = v;
                self.cfg.rating.safe = v;
                self.schedule_save()
            }
            Message::RatingQuestionable(v) => {
                self.settings_form.rating_questionable = v;
                self.cfg.rating.questionable = v;
                self.schedule_save()
            }
            Message::RatingExplicit(v) => {
                self.settings_form.rating_explicit = v;
                self.cfg.rating.explicit = v;
                self.schedule_save()
            }
            Message::SkipVideo(v) => {
                self.settings_form.skip_video = v;
                self.cfg.media_skip.video = v;
                self.schedule_save()
            }
            Message::SkipFlash(v) => {
                self.settings_form.skip_flash = v;
                self.cfg.media_skip.flash = v;
                self.schedule_save()
            }
            Message::SkipAnimation(v) => {
                self.settings_form.skip_animation = v;
                self.cfg.media_skip.animation = v;
                self.schedule_save()
            }
            Message::DownloadLimitChanged(limit) => {
                self.settings_form.download_limit = limit;
                self.cfg.max_posts_per_run = limit.into_limit();
                self.schedule_save()
            }
            Message::BlacklistEdited(action) => {
                self.blacklist_content.perform(action);
                self.update_blacklist(self.blacklist_content.text())
            }
            Message::ConfigSaveTick(token) => {
                if token == self.save_token {
                    self.persist_config_now();
                }
                Task::none()
            }

            Message::Login => {
                if self.creds.is_empty() || self.creds_checking {
                    return Task::none();
                }
                self.login_token = self.login_token.wrapping_add(1);
                let token = self.login_token;
                self.creds_checking = true;
                self.settings_form.creds_checking = true;
                self.settings_form.creds_error.clear();
                let creds = self.creds.clone();
                let checked_creds = creds.clone();
                let site = self.cfg.site;
                Task::perform(
                    async move {
                        let client = Client::new(site, Some(creds))
                            .await
                            .map_err(|e| format!("{e}"))?;
                        client.verify_login().await.map_err(|e| format!("{e:#}"))
                    },
                    move |result| Message::LoginCompleted {
                        token,
                        creds: checked_creds,
                        result,
                    },
                )
            }
            Message::LoginCompleted {
                token,
                creds,
                result,
            } => {
                if token != self.login_token {
                    return Task::none();
                }
                self.creds_checking = false;
                self.settings_form.creds_checking = false;
                match result {
                    Ok(()) => match crate::credentials::save(&creds) {
                        Ok(()) => {
                            self.creds = creds.clone();
                            self.active_creds = Some(creds.clone());
                            self.settings_form.creds_loaded = true;
                            self.settings_form.username = creds.username;
                            self.settings_form.api_key.clear();
                            self.settings_form.api_key_saved = true;
                            self.settings_form.creds_dirty = false;
                            self.settings_form.creds_error.clear();
                            self.push_log(
                                LogLevel::Info,
                                "logged in, credentials saved to OS keyring",
                            );
                        }
                        Err(e) => {
                            self.settings_form.creds_error = format!("{e}");
                            self.push_log(LogLevel::Error, format!("credentials save failed: {e}"));
                        }
                    },
                    Err(err) => {
                        self.settings_form.creds_loaded = self.active_creds.is_some();
                        self.settings_form.creds_error = err.clone();
                        self.push_log(LogLevel::Error, format!("login failed: {err}"));
                    }
                }
                Task::none()
            }
            Message::Logout => {
                self.invalidate_login_attempt();
                self.creds = Credentials::default();
                self.active_creds = None;
                self.settings_form.username.clear();
                self.settings_form.api_key.clear();
                self.settings_form.api_key_saved = false;
                self.settings_form.creds_loaded = false;
                self.settings_form.creds_dirty = false;
                self.settings_form.creds_error.clear();
                if let Err(e) = crate::credentials::clear() {
                    self.push_log(LogLevel::Warn, format!("logout: {e}"));
                } else {
                    self.push_log(LogLevel::Info, "logged out");
                }
                Task::none()
            }

            Message::ClearLogs => {
                self.log_lines.clear();
                Task::none()
            }

            Message::DownloadEvent(ev) => {
                self.handle_download_event(ev);
                Task::none()
            }
            Message::PhaseTick => {
                self.phase_tick = (self.phase_tick + 1) % 4;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let active_jobs = self.jobs.values().filter(|j| !j.finished).count() as u32;
        let queries: Vec<QueryView> = self
            .cfg
            .queries
            .iter()
            .map(|q| self.to_query_view(q))
            .collect();
        let mut sorted_jobs: Vec<(&u64, &JobState)> = self.jobs.iter().collect();
        sorted_jobs.sort_by_key(|(id, _)| std::cmp::Reverse(**id));
        let jobs: Vec<JobView> = sorted_jobs
            .iter()
            .map(|(id, j)| self.to_job_view(**id, j))
            .collect();
        let logs: Vec<LogLine> = self.log_lines.iter().cloned().collect();

        let main: Element<Message> = match self.active_tab {
            Tab::Queue => view::queue::view(
                queries,
                jobs,
                &self.new_query_buf,
                self.active_creds.is_some(),
                self.removed_query
                    .as_ref()
                    .map(|(_, query)| query.tags.as_str()),
            ),
            Tab::Settings => view::settings::view(&self.settings_form, &self.blacklist_content),
            Tab::Log => view::log::view(logs),
        };

        let layout = row![view::sidebar::view(self.active_tab, active_jobs), main];

        container(layout)
            .style(theme::page_bg)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn schedule_save(&mut self) -> Task<Message> {
        self.save_token = self.save_token.wrapping_add(1);
        let token = self.save_token;
        Task::perform(
            async move {
                tokio::time::sleep(SAVE_DEBOUNCE).await;
                token
            },
            Message::ConfigSaveTick,
        )
    }

    fn persist_config_now(&mut self) {
        match self.cfg.save(&self.cfg_path) {
            Ok(()) => self.settings_form.config_save_error.clear(),
            Err(e) => {
                self.settings_form.config_save_error = format!("{e}");
                self.push_log(LogLevel::Error, format!("config save failed: {e}"));
            }
        }
    }

    fn update_blacklist(&mut self, text: String) -> Task<Message> {
        self.settings_form.blacklist = text.clone();
        self.cfg.blacklist = parse_blacklist(&text);
        self.schedule_save()
    }

    fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.log_lines.len() >= LOG_LINE_CAP {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(LogLine {
            level,
            timestamp: time::OffsetDateTime::now_utc()
                .format(time::macros::format_description!(
                    "[hour]:[minute]:[second]"
                ))
                .unwrap_or_default(),
            text: text.into(),
        });
    }

    fn invalidate_login_attempt(&mut self) {
        if self.creds_checking {
            self.login_token = self.login_token.wrapping_add(1);
            self.creds_checking = false;
            self.settings_form.creds_checking = false;
        }
    }

    fn sync_credentials_dirty(&mut self) {
        self.settings_form.creds_dirty = self.active_creds.as_ref() != Some(&self.creds);
    }

    fn start_job(&mut self, tags: String) {
        if self.active_creds.is_none() {
            self.push_log(LogLevel::Warn, "login required before downloading");
            return;
        }
        let tags = tags.trim().to_string();
        if tags.is_empty() {
            return;
        }

        if !self.cfg.queries.iter().any(|q| q.tags == tags) {
            self.cfg.new_query(tags.clone());
            self.persist_config_now();
        }

        let already_running = self.jobs.values().any(|j| !j.finished && j.tags == tags);
        let already_pending = self.pending_tags.iter().any(|t| t == &tags);
        if already_running || already_pending {
            self.push_log(LogLevel::Warn, format!("already running/queued: {tags}"));
            return;
        }

        let any_active = self.jobs.values().any(|j| !j.finished);
        if any_active {
            self.pending_tags.push_back(tags.clone());
            self.push_log(LogLevel::Info, format!("queued: {tags}"));
            return;
        }

        self.spawn_now(tags);
    }

    fn spawn_now(&mut self, tags: String) {
        let cfg = self.cfg.clone();
        let Some(creds) = self.active_creds.clone() else {
            self.push_log(LogLevel::Warn, "login required before downloading");
            return;
        };
        let creds = Some(creds);
        let handle = self.manager.spawn_job(tags.clone(), cfg, creds);
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
                discovering: true,
                finished: false,
            },
        );
    }

    fn drain_pending(&mut self) {
        while let Some(next) = self.pending_tags.pop_front() {
            if self.jobs.values().any(|j| !j.finished && j.tags == next) {
                continue;
            }
            self.spawn_now(next);
            break;
        }
    }

    fn handle_download_event(&mut self, ev: DownloadEvent) {
        match ev {
            DownloadEvent::JobStarted { job_id, tags } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Starting;
                }
                self.push_log(LogLevel::Info, format!("started: {tags}"));
            }
            DownloadEvent::Discovering {
                job_id,
                pages_scanned,
                posts_queued,
            } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    if j.phase == JobPhase::Starting {
                        j.phase = JobPhase::Discovering;
                    }
                    j.pages_scanned = pages_scanned;
                    j.total = posts_queued;
                }
            }
            DownloadEvent::DiscoveryDone {
                job_id,
                total_posts,
                skipped_existing,
                skipped_failed,
                limit_reached,
            } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.discovering = false;
                    j.total = total_posts;
                    if total_posts > 0
                        && matches!(j.phase, JobPhase::Starting | JobPhase::Discovering)
                    {
                        j.phase = JobPhase::Downloading;
                    }
                }
                self.push_log(
                    LogLevel::Info,
                    format!(
                        "discovery done: {total_posts} to download, {skipped_existing} existing, {skipped_failed} previously failed/unavailable{}",
                        if limit_reached {
                            ", per-run limit reached"
                        } else {
                            ""
                        }
                    ),
                );
            }
            DownloadEvent::Progress {
                job_id,
                done,
                failed,
                current,
                bytes_per_sec,
            } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    if matches!(j.phase, JobPhase::Starting | JobPhase::Discovering) {
                        j.phase = JobPhase::Downloading;
                    }
                    j.done = done;
                    j.failed = failed;
                    j.current_file = current;
                    j.bytes_per_sec = bytes_per_sec;
                }
            }
            DownloadEvent::PostFailed { post_id, error } => {
                self.push_log(LogLevel::Warn, format!("post {post_id} failed: {error}"));
            }
            DownloadEvent::JobFinished {
                job_id,
                done,
                failed,
                total,
                duration_ms,
            } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Finished;
                    j.finished = true;
                    j.discovering = false;
                    j.done = done;
                    j.failed = failed;
                    j.total = total;
                    j.current_file = None;
                    j.handle = None;
                }
                self.push_log(
                    LogLevel::Info,
                    format!(
                        "finished: {done}/{total} ok, {failed} failed, {:.1}s",
                        duration_ms as f64 / 1000.0
                    ),
                );
                self.prune_finished_jobs();
                self.drain_pending();
            }
            DownloadEvent::JobCancelled { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Cancelled;
                    j.phase_before_pause = None;
                    j.finished = true;
                    j.discovering = false;
                    j.current_file = None;
                    j.bytes_per_sec = 0;
                    j.handle = None;
                }
                self.push_log(LogLevel::Warn, "cancelled");
                self.prune_finished_jobs();
                self.drain_pending();
            }
            DownloadEvent::JobPaused { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id)
                    && j.phase != JobPhase::Paused
                {
                    j.phase_before_pause = Some(j.phase);
                    j.phase = JobPhase::Paused;
                }
                self.push_log(LogLevel::Info, "paused");
            }
            DownloadEvent::JobResumed { job_id } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = j.phase_before_pause.take().unwrap_or(JobPhase::Downloading);
                    j.current_file = None;
                    j.bytes_per_sec = 0;
                }
                self.push_log(LogLevel::Info, "resumed");
            }
            DownloadEvent::JobError { job_id, error } => {
                if let Some(j) = self.jobs.get_mut(&job_id) {
                    j.phase = JobPhase::Errored;
                    j.finished = true;
                    j.discovering = false;
                    j.current_file = None;
                    j.bytes_per_sec = 0;
                    j.handle = None;
                }
                self.push_log(LogLevel::Error, format!("error: {error}"));
                self.prune_finished_jobs();
                self.drain_pending();
            }
        }
    }

    fn prune_finished_jobs(&mut self) {
        let mut finished: Vec<u64> = self
            .jobs
            .iter()
            .filter_map(|(id, job)| job.finished.then_some(*id))
            .collect();
        if finished.len() <= FINISHED_JOB_CAP {
            return;
        }
        finished.sort_unstable();
        let remove_count = finished.len() - FINISHED_JOB_CAP;
        for id in finished.into_iter().take(remove_count) {
            self.jobs.remove(&id);
        }
    }

    fn to_query_view(&self, q: &TagQuery) -> QueryView {
        let st = self.state_store.get(&q.tags);
        QueryView {
            id: q.id,
            tags: q.tags.clone(),
            failed_count: st.failed.len(),
            running: self.jobs.values().any(|j| !j.finished && j.tags == q.tags),
            queued: self.pending_tags.iter().any(|t| t == &q.tags),
            last_run: st.last_run.map(format_last_run).unwrap_or_default(),
        }
    }

    fn to_job_view(&self, id: u64, j: &JobState) -> JobView {
        let (phase_label, color_idx) = match j.phase {
            JobPhase::Starting => ("starting", 0),
            JobPhase::Discovering => ("discovering", 1),
            JobPhase::Downloading => ("downloading", 1),
            JobPhase::Paused => ("paused", 3),
            JobPhase::Finished => ("done", 2),
            JobPhase::Cancelled => ("cancelled", 3),
            JobPhase::Errored => ("error", 4),
        };
        let dots = if matches!(
            j.phase,
            JobPhase::Starting | JobPhase::Discovering | JobPhase::Downloading
        ) {
            ".".repeat(self.phase_tick as usize)
        } else {
            String::new()
        };
        let progress = match j.phase {
            JobPhase::Starting | JobPhase::Discovering => 0.0,
            JobPhase::Downloading | JobPhase::Paused if j.total > 0 => {
                (j.done as f32 / j.total as f32).clamp(0.0, 1.0)
            }
            JobPhase::Downloading | JobPhase::Paused => 0.0,
            JobPhase::Finished => 1.0,
            JobPhase::Cancelled | JobPhase::Errored if j.total > 0 => {
                (j.done as f32 / j.total as f32).clamp(0.0, 1.0)
            }
            JobPhase::Cancelled | JobPhase::Errored => 0.0,
        };
        JobView {
            id,
            tags: j.tags.clone(),
            phase_color_idx: color_idx,
            phase_label: phase_label.to_string(),
            phase_dots: dots,
            stats_label: format_stats(j),
            progress,
            current_file: j.current_file.clone().unwrap_or_default(),
            finished: j.finished,
            paused: j.phase == JobPhase::Paused,
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for job in self.jobs.values_mut().filter(|j| !j.finished) {
            if let Some(handle) = job.handle.take() {
                handle.cancel();
            }
        }
        self.pending_tags.clear();
    }
}

fn format_stats(j: &JobState) -> String {
    let speed = format_bps(j.bytes_per_sec);
    match j.phase {
        JobPhase::Starting | JobPhase::Discovering => {
            format!("{} pages · {} queued", j.pages_scanned, j.total)
        }
        JobPhase::Downloading => {
            let total = if j.discovering {
                format!("{}+", j.total)
            } else {
                j.total.to_string()
            };
            format!("{}/{} · {} failed · {}", j.done, total, j.failed, speed)
        }
        JobPhase::Paused => {
            let was = j.phase_before_pause.unwrap_or(JobPhase::Downloading);
            match was {
                JobPhase::Starting | JobPhase::Discovering => {
                    format!("{} pages · {} queued · paused", j.pages_scanned, j.total)
                }
                _ => format!("{}/{} · {} failed · paused", j.done, j.total, j.failed),
            }
        }
        JobPhase::Finished | JobPhase::Cancelled => {
            format!("{}/{} · {} failed", j.done, j.total, j.failed)
        }
        JobPhase::Errored => String::new(),
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

fn format_last_run(timestamp: i64) -> String {
    let Ok(value) = time::OffsetDateTime::from_unix_timestamp(timestamp) else {
        return String::new();
    };
    value
        .format(time::macros::format_description!(
            "[year]-[month]-[day] [hour]:[minute] UTC"
        ))
        .unwrap_or_default()
}

fn build_settings_form(
    cfg: &Config,
    creds: &Credentials,
    creds_loaded: bool,
    creds_err: Option<&str>,
) -> SettingsForm {
    SettingsForm {
        username: creds.username.clone(),
        api_key: if creds_loaded {
            String::new()
        } else {
            creds.api_key.clone()
        },
        api_key_saved: creds_loaded,
        creds_loaded,
        creds_checking: false,
        creds_error: creds_err.unwrap_or_default().to_string(),
        creds_dirty: false,
        config_save_error: String::new(),
        site: SiteOption::from_site(cfg.site),
        download_dir: cfg.download_dir.display().to_string(),
        rating_safe: cfg.rating.safe,
        rating_questionable: cfg.rating.questionable,
        rating_explicit: cfg.rating.explicit,
        skip_video: cfg.media_skip.video,
        skip_flash: cfg.media_skip.flash,
        skip_animation: cfg.media_skip.animation,
        download_limit: DownloadLimitOption::from_limit(cfg.max_posts_per_run),
        blacklist: cfg.blacklist.join("\n"),
    }
}

fn parse_blacklist(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn keyboard_shortcut(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key,
        physical_key,
        modifiers,
        repeat,
        ..
    }) = event
    else {
        return None;
    };
    if repeat || !modifiers.command() {
        return None;
    }
    match key.to_latin(physical_key) {
        Some('1') => Some(Message::TabSelected(Tab::Queue)),
        Some('2') => Some(Message::TabSelected(Tab::Settings)),
        Some('3') => Some(Message::TabSelected(Tab::Log)),
        Some('o') => Some(Message::OpenDownloadFolder),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn open_download_folder(path: &std::path::Path) -> Result<(), String> {
    open_download_folder_with(path, "open")
}

#[cfg(target_os = "windows")]
fn open_download_folder(path: &std::path::Path) -> Result<(), String> {
    open_download_folder_with(path, "explorer")
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn open_download_folder(path: &std::path::Path) -> Result<(), String> {
    open_download_folder_with(path, "xdg-open")
}

fn open_download_folder_with(path: &std::path::Path, program: &str) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|err| format!("create `{}`: {err}", path.display()))?;
    let status = std::process::Command::new(program)
        .arg(path)
        .status()
        .map_err(|err| format!("start {program}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{App, FINISHED_JOB_CAP, JobPhase, JobState, Message, parse_blacklist};
    use crate::config::{Config, TagQuery};
    use crate::download::{DownloadEvent, DownloadManager};
    use crate::state::StateStore;
    use std::collections::{HashMap, VecDeque};
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn parse_blacklist_trims_and_drops_empty_lines() {
        assert_eq!(
            parse_blacklist(" young\n\n -animated \n\tflash\t"),
            vec!["young", "-animated", "flash"]
        );
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("feline-app-test-{name}-{}", std::process::id()))
    }

    fn app_for_test(state_name: &str) -> App {
        let state_path = temp_path(state_name);
        let _ = std::fs::remove_file(&state_path);
        let state_store = StateStore::load(&state_path);
        let (manager, _rx) =
            DownloadManager::new(tokio::runtime::Handle::current(), state_store.clone());
        let cfg = Config {
            queries: vec![TagQuery {
                id: 1,
                tags: "cat".into(),
            }],
            ..Config::default()
        };
        let creds = crate::credentials::Credentials {
            username: "user".into(),
            api_key: "key".into(),
        };
        let cfg_path = temp_path(&format!("config-{state_name}"));
        let _ = std::fs::remove_file(&cfg_path);
        let settings_form = super::build_settings_form(&cfg, &creds, true, None);
        App {
            cfg: cfg.clone(),
            cfg_path,
            save_token: 0,
            creds: creds.clone(),
            active_creds: Some(creds),
            creds_checking: false,
            login_token: 0,
            state_store,
            manager: Arc::new(manager),
            jobs: HashMap::new(),
            pending_tags: VecDeque::new(),
            log_lines: VecDeque::new(),
            removed_query: None,
            active_tab: super::Tab::Queue,
            new_query_buf: String::new(),
            settings_form,
            blacklist_content: iced::widget::text_editor::Content::new(),
            phase_tick: 0,
        }
    }

    fn active_job(tags: &str, phase: JobPhase) -> JobState {
        JobState {
            tags: tags.into(),
            phase,
            phase_before_pause: None,
            pages_scanned: 0,
            total: 0,
            done: 0,
            failed: 0,
            current_file: None,
            bytes_per_sec: 0,
            handle: None,
            discovering: true,
            finished: false,
        }
    }

    #[tokio::test]
    async fn cancel_query_removes_pending_entry() {
        let mut app = app_for_test("cancel-pending");
        app.pending_tags.push_back("cat".into());

        let _ = app.update(Message::CancelQuery(1));

        assert!(app.pending_tags.is_empty());
    }

    #[tokio::test]
    async fn clear_query_failures_empties_failed_state() {
        let mut app = app_for_test("clear-failures");
        app.state_store.update("cat", |s| {
            s.failed.insert(10);
            s.failed.insert(20);
        });

        let _ = app.update(Message::ClearQueryFailures(1));

        assert!(app.state_store.get("cat").failed.is_empty());
    }

    #[tokio::test]
    async fn pause_and_resume_events_restore_previous_phase() {
        let mut app = app_for_test("pause-resume");
        app.jobs.insert(7, active_job("cat", JobPhase::Downloading));

        app.handle_download_event(DownloadEvent::JobPaused { job_id: 7 });
        assert_eq!(app.jobs.get(&7).unwrap().phase, JobPhase::Paused);
        assert_eq!(
            app.jobs.get(&7).unwrap().phase_before_pause,
            Some(JobPhase::Downloading)
        );

        app.handle_download_event(DownloadEvent::JobResumed { job_id: 7 });
        assert_eq!(app.jobs.get(&7).unwrap().phase, JobPhase::Downloading);
        assert_eq!(app.jobs.get(&7).unwrap().phase_before_pause, None);
    }

    #[tokio::test]
    async fn job_error_marks_job_finished_and_releases_handle() {
        let mut app = app_for_test("job-error");
        app.jobs.insert(9, active_job("cat", JobPhase::Discovering));

        app.handle_download_event(DownloadEvent::JobError {
            job_id: 9,
            error: "network".into(),
        });

        let job = app.jobs.get(&9).unwrap();
        assert_eq!(job.phase, JobPhase::Errored);
        assert!(job.finished);
        assert!(!job.discovering);
        assert!(job.handle.is_none());
    }

    #[tokio::test]
    async fn credential_edits_leave_verified_credentials_active() {
        let mut app = app_for_test("credential-draft");

        let _ = app.update(Message::UsernameChanged("new-user".into()));

        assert_eq!(app.creds.username, "new-user");
        assert_eq!(
            app.active_creds
                .as_ref()
                .map(|creds| creds.username.as_str()),
            Some("user")
        );
        assert!(app.settings_form.creds_dirty);
    }

    #[tokio::test]
    async fn stale_login_result_cannot_replace_verified_credentials() {
        let mut app = app_for_test("stale-login");
        app.login_token = 2;
        let stale = crate::credentials::Credentials {
            username: "stale-user".into(),
            api_key: "stale-key".into(),
        };

        let _ = app.update(Message::LoginCompleted {
            token: 1,
            creds: stale,
            result: Ok(()),
        });

        assert_eq!(
            app.active_creds
                .as_ref()
                .map(|creds| creds.username.as_str()),
            Some("user")
        );
    }

    #[tokio::test]
    async fn removed_query_can_be_restored() {
        let mut app = app_for_test("undo-remove");

        let _ = app.update(Message::RemoveQuery(1));
        assert!(app.cfg.queries.is_empty());

        let _ = app.update(Message::UndoRemoveQuery);
        assert_eq!(app.cfg.queries.len(), 1);
        assert_eq!(app.cfg.queries[0].tags, "cat");
    }

    #[tokio::test]
    async fn clear_finished_jobs_preserves_active_jobs() {
        let mut app = app_for_test("clear-finished");
        let mut finished = active_job("cat", JobPhase::Finished);
        finished.finished = true;
        app.jobs.insert(1, finished);
        app.jobs.insert(2, active_job("dog", JobPhase::Downloading));

        let _ = app.update(Message::ClearFinishedJobs);

        assert!(!app.jobs.contains_key(&1));
        assert!(app.jobs.contains_key(&2));
    }

    #[tokio::test]
    async fn finished_job_history_is_bounded() {
        let mut app = app_for_test("bounded-history");
        for id in 1..=(FINISHED_JOB_CAP as u64 + 5) {
            let mut job = active_job("cat", JobPhase::Finished);
            job.finished = true;
            app.jobs.insert(id, job);
        }

        app.prune_finished_jobs();

        assert_eq!(app.jobs.len(), FINISHED_JOB_CAP);
        assert!(!app.jobs.contains_key(&1));
    }
}
