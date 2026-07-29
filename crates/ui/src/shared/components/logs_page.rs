//! Diagnostics log page helpers and renderer.

use crate::shared::components::network_log::{NetworkLog, NetworkLogState};
use crate::shared::theme::ThemeColors;
use eframe::egui::{self, RichText, Ui};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

const MAX_LOG_BYTES: u64 = 512 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
pub const LOG_ROW_HEIGHT: f32 = 22.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFileSnapshot {
    pub path: PathBuf,
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFileEntry {
    pub path: PathBuf,
    pub label: String,
    modified: SystemTime,
}

#[derive(Debug, Clone)]
pub struct LogCatalog {
    pub app_files: Vec<LogFileEntry>,
    pub plugin_files: Vec<LogFileEntry>,
    pub refreshed_at: Instant,
}

#[derive(Debug, Clone)]
pub struct LogSession {
    pub app_log_path: PathBuf,
    pub app_log_offset: u64,
    pub plugin_log_dir: PathBuf,
    plugin_offsets: HashMap<PathBuf, u64>,
}

impl LogSession {
    /// Captures a session against the application's default log
    /// locations.
    ///
    /// `AppPaths::system_default` is pure path arithmetic — it resolves
    /// the same directories `ArclainApp::bootstrap` will, without
    /// creating anything — so this can run *before* the application is
    /// bootstrapped. That ordering matters: the captured offsets mark
    /// where the "current session" view starts, so taking them first is
    /// what makes the application's own startup logging visible there.
    pub fn capture_default() -> Self {
        match arclain_app::AppPaths::system_default() {
            Ok(paths) => Self::capture(paths.current_app_log_file(), paths.plugin_log_dir()),
            Err(error) => {
                // Not reachable today (`system_default` falls back to the
                // working directory when the OS cannot name a home), but
                // a diagnostics page that cannot find its files should
                // degrade to showing a read error, never take startup
                // down with it.
                tracing::warn!("Could not resolve the application log locations: {error:?}");
                Self::capture_with_app_offset(PathBuf::new(), 0, PathBuf::new())
            }
        }
    }

    pub fn capture(app_log_path: PathBuf, plugin_log_dir: impl AsRef<Path>) -> Self {
        let app_log_offset = file_len(&app_log_path);
        Self::capture_with_app_offset(app_log_path, app_log_offset, plugin_log_dir)
    }

    pub fn capture_with_app_offset(
        app_log_path: PathBuf,
        app_log_offset: u64,
        plugin_log_dir: impl AsRef<Path>,
    ) -> Self {
        let plugin_log_dir = plugin_log_dir.as_ref().to_path_buf();
        let plugin_offsets = capture_plugin_offsets(&plugin_log_dir);

        Self {
            app_log_path,
            app_log_offset,
            plugin_log_dir,
            plugin_offsets,
        }
    }

    fn app_log_dir(&self) -> PathBuf {
        self.app_log_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Filesystem operations owned by the log worker.
///
/// This is public only so integration tests can supply deterministic blocked
/// and counting adapters. Render code interacts with [`LogWorker`] instead.
#[doc(hidden)]
pub trait LogIo: Send + Sync + 'static {
    fn list_app_files(&self, dir: &Path) -> io::Result<Vec<LogFileEntry>>;
    fn list_plugin_files(&self, dir: &Path) -> io::Result<Vec<LogFileEntry>>;
    fn read_file(&self, path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot>;
    fn read_session(&self, path: &Path, offset: u64, max_bytes: u64)
        -> io::Result<LogFileSnapshot>;
    fn read_plugin_session(
        &self,
        session: &LogSession,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot>;
}

struct FilesystemLogIo;

impl LogIo for FilesystemLogIo {
    fn list_app_files(&self, dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        list_app_log_files(dir)
    }

    fn list_plugin_files(&self, dir: &Path) -> io::Result<Vec<LogFileEntry>> {
        list_plugin_log_files(dir)
    }

    fn read_file(&self, path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot> {
        read_log_tail(path, max_bytes)
    }

    fn read_session(
        &self,
        path: &Path,
        offset: u64,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        read_log_tail_from_offset(path, offset, max_bytes)
    }

    fn read_plugin_session(
        &self,
        session: &LogSession,
        max_bytes: u64,
    ) -> io::Result<LogFileSnapshot> {
        read_plugin_session_logs(session, max_bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogRequestId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogWorkerUnavailable;

impl std::fmt::Display for LogWorkerUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("log worker request channel is unavailable")
    }
}

impl std::error::Error for LogWorkerUnavailable {}

#[derive(Debug, Clone)]
pub enum LogRequest {
    ListApp {
        dir: PathBuf,
    },
    ListPlugins {
        dir: PathBuf,
    },
    ReadFile {
        path: PathBuf,
        max_bytes: u64,
    },
    ReadSession {
        path: PathBuf,
        offset: u64,
        max_bytes: u64,
    },
    ReadPluginSession {
        session: LogSession,
        max_bytes: u64,
    },
}

#[derive(Debug)]
pub enum LogResult {
    AppFiles {
        request_id: LogRequestId,
        files: Result<Vec<LogFileEntry>, String>,
    },
    PluginFiles {
        request_id: LogRequestId,
        files: Result<Vec<LogFileEntry>, String>,
    },
    Snapshot {
        request_id: LogRequestId,
        snapshot: Result<LogFileSnapshot, String>,
    },
}

struct LogJob {
    request_id: LogRequestId,
    request: LogRequest,
    ctx: egui::Context,
}

pub struct LogWorker {
    sender: mpsc::Sender<LogJob>,
    receiver: Mutex<mpsc::Receiver<LogResult>>,
    next_id: AtomicU64,
}

impl LogWorker {
    fn new() -> Self {
        Self::with_io(Arc::new(FilesystemLogIo))
    }

    #[doc(hidden)]
    pub fn with_io(io: Arc<dyn LogIo>) -> Self {
        let (job_sender, job_receiver) = mpsc::channel::<LogJob>();
        let (result_sender, result_receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("arclain-log-worker".to_string())
            .spawn(move || {
                while let Ok(job) = job_receiver.recv() {
                    let result = execute_log_request(io.as_ref(), job.request_id, job.request);
                    if result_sender.send(result).is_err() {
                        break;
                    }
                    job.ctx.request_repaint();
                }
            })
            .expect("failed to start log worker");

        Self {
            sender: job_sender,
            receiver: Mutex::new(result_receiver),
            next_id: AtomicU64::new(0),
        }
    }

    pub fn request(
        &self,
        request: LogRequest,
        ctx: egui::Context,
    ) -> Result<LogRequestId, LogWorkerUnavailable> {
        let request_id = LogRequestId(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        if self
            .sender
            .send(LogJob {
                request_id,
                request,
                ctx,
            })
            .is_err()
        {
            tracing::error!("log worker request channel is unavailable");
            return Err(LogWorkerUnavailable);
        }
        Ok(request_id)
    }

    pub fn drain(&self) -> Vec<LogResult> {
        let receiver = self.receiver.lock();
        let mut results = Vec::new();
        while let Ok(result) = receiver.try_recv() {
            results.push(result);
        }
        results
    }
}

fn execute_log_request(io: &dyn LogIo, request_id: LogRequestId, request: LogRequest) -> LogResult {
    match request {
        LogRequest::ListApp { dir } => LogResult::AppFiles {
            request_id,
            files: io.list_app_files(&dir).map_err(|error| {
                format!("Failed to list app logs from {}: {error}", dir.display())
            }),
        },
        LogRequest::ListPlugins { dir } => LogResult::PluginFiles {
            request_id,
            files: io.list_plugin_files(&dir).map_err(|error| {
                format!("Failed to list plugin logs from {}: {error}", dir.display())
            }),
        },
        LogRequest::ReadFile { path, max_bytes } => LogResult::Snapshot {
            request_id,
            snapshot: io
                .read_file(&path, max_bytes)
                .map_err(|error| format!("Failed to read {}: {error}", path.display())),
        },
        LogRequest::ReadSession {
            path,
            offset,
            max_bytes,
        } => LogResult::Snapshot {
            request_id,
            snapshot: io
                .read_session(&path, offset, max_bytes)
                .map_err(|error| format!("Failed to read {}: {error}", path.display())),
        },
        LogRequest::ReadPluginSession { session, max_bytes } => LogResult::Snapshot {
            request_id,
            snapshot: io
                .read_plugin_session(&session, max_bytes)
                .map_err(|error| {
                    format!(
                        "Failed to read plugin logs from {}: {error}",
                        session.plugin_log_dir.display()
                    )
                }),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsTab {
    App,
    Plugins,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LogSource {
    CurrentSession,
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LogReadKey {
    File(PathBuf),
    SessionFile(PathBuf, u64),
    PluginSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingLogRead {
    request_id: LogRequestId,
    key: LogReadKey,
}

pub struct FileLogViewState {
    pub filter: String,
    pub auto_scroll: bool,
    snapshot: Option<Result<LogFileSnapshot, String>>,
    snapshot_key: Option<LogReadKey>,
    pending: Option<PendingLogRead>,
    last_refresh: Option<Instant>,
    snapshot_revision: u64,
    filtered_revision: u64,
    filtered_filter: String,
    filtered_indices: Vec<usize>,
}

impl FileLogViewState {
    fn new() -> Self {
        Self {
            filter: String::new(),
            auto_scroll: true,
            snapshot: None,
            snapshot_key: None,
            pending: None,
            last_refresh: None,
            snapshot_revision: 0,
            filtered_revision: u64::MAX,
            filtered_filter: String::new(),
            filtered_indices: Vec::new(),
        }
    }

    fn should_request(&self, key: &LogReadKey, force: bool) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| &pending.key == key)
        {
            return false;
        }
        force
            || self.snapshot_key.as_ref() != Some(key)
            || self
                .last_refresh
                .is_none_or(|last| last.elapsed() >= REFRESH_INTERVAL)
    }

    fn begin_request(&mut self, key: LogReadKey, request_id: LogRequestId) {
        if self.snapshot_key.as_ref() != Some(&key) {
            self.snapshot = None;
            self.filtered_indices.clear();
            self.filtered_revision = u64::MAX;
        }
        self.pending = Some(PendingLogRead { request_id, key });
    }

    fn pending_request_id(&self) -> Option<LogRequestId> {
        self.pending.as_ref().map(|pending| pending.request_id)
    }

    fn apply_snapshot(
        &mut self,
        request_id: LogRequestId,
        snapshot: Result<LogFileSnapshot, String>,
    ) -> bool {
        if self.pending_request_id() != Some(request_id) {
            return false;
        }
        let pending = self.pending.take().expect("matching pending log read");
        self.snapshot = Some(snapshot);
        self.snapshot_key = Some(pending.key);
        self.last_refresh = Some(Instant::now());
        self.snapshot_revision = self.snapshot_revision.wrapping_add(1).max(1);
        self.filtered_revision = u64::MAX;
        true
    }

    fn update_filtered_indices(&mut self) {
        let normalized_filter = self.filter.to_lowercase();
        if self.filtered_revision == self.snapshot_revision
            && self.filtered_filter == normalized_filter
        {
            return;
        }

        self.filtered_indices.clear();
        if let Some(Ok(snapshot)) = &self.snapshot {
            self.filtered_indices.extend(
                snapshot
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| {
                        normalized_filter.is_empty()
                            || line.to_lowercase().contains(&normalized_filter)
                    })
                    .map(|(index, _)| index),
            );
        }
        self.filtered_revision = self.snapshot_revision;
        self.filtered_filter = normalized_filter;
    }
}

pub struct LogsPageState {
    pub active_tab: LogsTab,
    pub app_log: FileLogViewState,
    pub plugin_log: FileLogViewState,
    pub network_log: NetworkLogState,
    session: LogSession,
    app_source: LogSource,
    plugin_source: LogSource,
    worker: LogWorker,
    catalog: LogCatalog,
    app_list_pending: Option<LogRequestId>,
    plugin_list_pending: Option<LogRequestId>,
    app_list_error: Option<String>,
    plugin_list_error: Option<String>,
}

impl LogsPageState {
    pub fn new() -> Self {
        Self::with_session(LogSession::capture_default())
    }

    pub fn with_session(session: LogSession) -> Self {
        Self::with_worker(session, LogWorker::new())
    }

    #[doc(hidden)]
    pub fn with_session_and_io(session: LogSession, io: Arc<dyn LogIo>) -> Self {
        Self::with_worker(session, LogWorker::with_io(io))
    }

    fn with_worker(session: LogSession, worker: LogWorker) -> Self {
        let now = Instant::now();
        Self {
            active_tab: LogsTab::App,
            app_log: FileLogViewState::new(),
            plugin_log: FileLogViewState::new(),
            network_log: NetworkLogState::new(),
            session,
            app_source: LogSource::CurrentSession,
            plugin_source: LogSource::CurrentSession,
            worker,
            catalog: LogCatalog {
                app_files: Vec::new(),
                plugin_files: Vec::new(),
                refreshed_at: now.checked_sub(REFRESH_INTERVAL).unwrap_or(now),
            },
            app_list_pending: None,
            plugin_list_pending: None,
            app_list_error: None,
            plugin_list_error: None,
        }
    }

    fn apply_completed(&mut self) {
        for result in self.worker.drain() {
            match result {
                LogResult::AppFiles { request_id, files }
                    if self.app_list_pending == Some(request_id) =>
                {
                    self.app_list_pending = None;
                    match files {
                        Ok(files) => {
                            self.catalog.app_files = files;
                            self.app_list_error = None;
                        }
                        Err(error) => self.app_list_error = Some(error),
                    }
                    self.catalog.refreshed_at = Instant::now();
                }
                LogResult::PluginFiles { request_id, files }
                    if self.plugin_list_pending == Some(request_id) =>
                {
                    self.plugin_list_pending = None;
                    match files {
                        Ok(files) => {
                            self.catalog.plugin_files = files;
                            self.plugin_list_error = None;
                        }
                        Err(error) => self.plugin_list_error = Some(error),
                    }
                    self.catalog.refreshed_at = Instant::now();
                }
                LogResult::Snapshot {
                    request_id,
                    snapshot,
                } => {
                    if self.app_log.pending_request_id() == Some(request_id) {
                        self.app_log.apply_snapshot(request_id, snapshot);
                    } else if self.plugin_log.pending_request_id() == Some(request_id) {
                        self.plugin_log.apply_snapshot(request_id, snapshot);
                    }
                }
                _ => {}
            }
        }
    }

    fn request_catalog_if_due(&mut self, ctx: &egui::Context) {
        let elapsed = self.catalog.refreshed_at.elapsed();
        if elapsed < REFRESH_INTERVAL {
            ctx.request_repaint_after(REFRESH_INTERVAL - elapsed);
            return;
        }
        if self.app_list_pending.is_none() {
            self.app_list_pending = self
                .worker
                .request(
                    LogRequest::ListApp {
                        dir: self.session.app_log_dir(),
                    },
                    ctx.clone(),
                )
                .ok();
        }
        if self.plugin_list_pending.is_none() {
            self.plugin_list_pending = self
                .worker
                .request(
                    LogRequest::ListPlugins {
                        dir: self.session.plugin_log_dir.clone(),
                    },
                    ctx.clone(),
                )
                .ok();
        }
    }
}

pub fn read_log_tail(path: &Path, max_bytes: u64) -> io::Result<LogFileSnapshot> {
    read_log_tail_from_offset(path, 0, max_bytes)
}

pub fn read_log_tail_from_offset(
    path: &Path,
    offset: u64,
    max_bytes: u64,
) -> io::Result<LogFileSnapshot> {
    read_log_tail_from_offset_with_bytes(path, offset, max_bytes).map(|(snapshot, _)| snapshot)
}

fn read_log_tail_from_offset_with_bytes(
    path: &Path,
    offset: u64,
    max_bytes: u64,
) -> io::Result<(LogFileSnapshot, u64)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    read_log_tail_from_reader(&mut file, path, len, offset, max_bytes)
}

fn read_log_tail_from_reader(
    reader: &mut (impl Read + Seek),
    path: &Path,
    len: u64,
    offset: u64,
    max_bytes: u64,
) -> io::Result<(LogFileSnapshot, u64)> {
    let offset = if offset > len { 0 } else { offset };
    let max_bytes = max_bytes.max(1);
    let available = len.saturating_sub(offset);
    let truncated = available > max_bytes;
    let start = if truncated { len - max_bytes } else { offset };

    reader.seek(SeekFrom::Start(start))?;

    let mut bytes = Vec::new();
    let bytes_read = reader.take(max_bytes).read_to_end(&mut bytes)? as u64;

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    if start > offset && !text.starts_with('\n') && !lines.is_empty() {
        lines.remove(0);
    }
    lines.retain(|line| !line.is_empty());

    Ok((
        LogFileSnapshot {
            path: path.to_path_buf(),
            lines,
            truncated,
        },
        bytes_read,
    ))
}

pub fn list_app_log_files(app_log_dir: &Path) -> io::Result<Vec<LogFileEntry>> {
    list_log_files(app_log_dir, |label| label.starts_with("arclain-"))
}

pub fn list_plugin_log_files(plugin_log_dir: &Path) -> io::Result<Vec<LogFileEntry>> {
    list_log_files(plugin_log_dir, |_| true)
}

pub fn read_plugin_session_logs(
    session: &LogSession,
    max_bytes: u64,
) -> io::Result<LogFileSnapshot> {
    let mut files = list_plugin_log_files(&session.plugin_log_dir)?;
    files.sort_by(|left, right| left.label.cmp(&right.label));

    let mut remaining = max_bytes.max(1);
    let mut lines = Vec::new();
    let mut truncated = false;

    for (index, file) in files.iter().enumerate() {
        if remaining == 0 {
            truncated = true;
            break;
        }

        let offset = session.plugin_offsets.get(&file.path).copied().unwrap_or(0);
        let (snapshot, bytes_read) =
            read_log_tail_from_offset_with_bytes(&file.path, offset, remaining)?;
        remaining = remaining.saturating_sub(bytes_read);
        truncated |= snapshot.truncated;

        for line in snapshot.lines {
            lines.push(format!("[{}] {}", file.label, line));
        }

        if remaining == 0 && index + 1 < files.len() {
            truncated = true;
        }
    }

    Ok(LogFileSnapshot {
        path: session.plugin_log_dir.clone(),
        lines,
        truncated,
    })
}

fn list_log_files<F>(dir: &Path, include: F) -> io::Result<Vec<LogFileEntry>>
where
    F: Fn(&str) -> bool,
{
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }

        let Some(label) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if !include(&label) {
            continue;
        }

        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push(LogFileEntry {
            path,
            label,
            modified,
        });
    }

    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.label.cmp(&right.label))
    });
    Ok(files)
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn capture_plugin_offsets(plugin_log_dir: &Path) -> HashMap<PathBuf, u64> {
    list_plugin_log_files(plugin_log_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|file| {
            let len = file_len(&file.path);
            (file.path, len)
        })
        .collect()
}

pub struct LogsPage;

impl LogsPage {
    pub fn render_page(
        ui: &mut Ui,
        network_entries: &[(SystemTime, String)],
        state: &mut LogsPageState,
        colors: &ThemeColors,
    ) {
        state.apply_completed();
        state.request_catalog_if_due(ui.ctx());

        ui.horizontal(|ui| {
            Self::tab_button(ui, state, LogsTab::App, "App");
            Self::tab_button(ui, state, LogsTab::Plugins, "Plugins");
            Self::tab_button(ui, state, LogsTab::Network, "Network");
        });
        ui.separator();

        match state.active_tab {
            LogsTab::App => Self::render_app_log(ui, state, colors),
            LogsTab::Plugins => Self::render_plugin_log(ui, state, colors),
            LogsTab::Network => {
                NetworkLog::render_page(ui, network_entries, &mut state.network_log, colors);
            }
        }
    }

    fn tab_button(ui: &mut Ui, state: &mut LogsPageState, tab: LogsTab, label: &str) {
        if ui
            .selectable_label(state.active_tab == tab, RichText::new(label).size(12.0))
            .clicked()
        {
            state.active_tab = tab;
        }
    }

    fn render_app_log(ui: &mut Ui, state: &mut LogsPageState, colors: &ThemeColors) {
        Self::render_source_combo(
            ui,
            "app_log_source",
            &mut state.app_source,
            &state.catalog.app_files,
        );
        if let Some(error) = &state.app_list_error {
            ui.label(RichText::new(error).size(11.0).color(colors.error));
        }

        let (key, request, empty_message) = match state.app_source.clone() {
            LogSource::CurrentSession => {
                let key = LogReadKey::SessionFile(
                    state.session.app_log_path.clone(),
                    state.session.app_log_offset,
                );
                let request = LogRequest::ReadSession {
                    path: state.session.app_log_path.clone(),
                    offset: state.session.app_log_offset,
                    max_bytes: MAX_LOG_BYTES,
                };
                (key, request, "No app log entries for this session")
            }
            LogSource::File(path) => {
                let key = LogReadKey::File(path.clone());
                let request = LogRequest::ReadFile {
                    path,
                    max_bytes: MAX_LOG_BYTES,
                };
                (key, request, "No entries in this app log")
            }
        };

        Self::queue_log_read(
            &state.worker,
            &mut state.app_log,
            key.clone(),
            request.clone(),
            ui.ctx(),
            false,
        );
        let refresh = Self::render_file_log(
            ui,
            "app_log_scroll",
            &mut state.app_log,
            colors,
            empty_message,
        );
        if refresh {
            Self::queue_log_read(
                &state.worker,
                &mut state.app_log,
                key,
                request,
                ui.ctx(),
                true,
            );
        }
    }

    fn render_plugin_log(ui: &mut Ui, state: &mut LogsPageState, colors: &ThemeColors) {
        Self::render_source_combo(
            ui,
            "plugin_log_source",
            &mut state.plugin_source,
            &state.catalog.plugin_files,
        );
        if let Some(error) = &state.plugin_list_error {
            ui.label(RichText::new(error).size(11.0).color(colors.error));
        }

        let (key, request, empty_message) = match state.plugin_source.clone() {
            LogSource::CurrentSession => {
                let key = LogReadKey::PluginSession;
                let request = LogRequest::ReadPluginSession {
                    session: state.session.clone(),
                    max_bytes: MAX_LOG_BYTES,
                };
                (key, request, "No plugin log entries for this session")
            }
            LogSource::File(path) => {
                let key = LogReadKey::File(path.clone());
                let request = LogRequest::ReadFile {
                    path,
                    max_bytes: MAX_LOG_BYTES,
                };
                (key, request, "No entries in this plugin log")
            }
        };

        Self::queue_log_read(
            &state.worker,
            &mut state.plugin_log,
            key.clone(),
            request.clone(),
            ui.ctx(),
            false,
        );
        let refresh = Self::render_file_log(
            ui,
            "plugin_log_scroll",
            &mut state.plugin_log,
            colors,
            empty_message,
        );
        if refresh {
            Self::queue_log_read(
                &state.worker,
                &mut state.plugin_log,
                key,
                request,
                ui.ctx(),
                true,
            );
        }
    }

    fn queue_log_read(
        worker: &LogWorker,
        state: &mut FileLogViewState,
        key: LogReadKey,
        request: LogRequest,
        ctx: &egui::Context,
        force: bool,
    ) {
        if state.should_request(&key, force) {
            if let Ok(request_id) = worker.request(request, ctx.clone()) {
                state.begin_request(key, request_id);
            }
        } else if !state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.key == key)
            && state.snapshot_key.as_ref() == Some(&key)
        {
            if let Some(last_refresh) = state.last_refresh {
                ctx.request_repaint_after(REFRESH_INTERVAL.saturating_sub(last_refresh.elapsed()));
            }
        }
    }

    fn render_source_combo(
        ui: &mut Ui,
        id: &'static str,
        source: &mut LogSource,
        files: &[LogFileEntry],
    ) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(id)
                .selected_text(source_label(source))
                .show_ui(ui, |ui| {
                    ui.selectable_value(source, LogSource::CurrentSession, "Current session");
                    for file in files {
                        ui.selectable_value(
                            source,
                            LogSource::File(file.path.clone()),
                            &file.label,
                        );
                    }
                });
        });
    }

    fn render_file_log(
        ui: &mut Ui,
        scroll_id: &'static str,
        state: &mut FileLogViewState,
        colors: &ThemeColors,
        empty_message: &str,
    ) -> bool {
        state.update_filtered_indices();
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(egui_phosphor::regular::FUNNEL).size(14.0));
            let filter_resp = ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text("Filter logs...")
                    .desired_width(220.0),
            );
            if filter_resp.changed() {
                state.auto_scroll = false;
                state.update_filtered_indices();
            }

            ui.separator();

            if ui
                .button(format!(
                    "{} Refresh",
                    egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                ))
                .clicked()
            {
                refresh = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(Ok(snapshot)) = &state.snapshot {
                    if ui
                        .button(format!("{} Copy", egui_phosphor::regular::COPY))
                        .clicked()
                    {
                        ui.ctx().copy_text(snapshot.lines.join("\n"));
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} entries{}",
                            state.filtered_indices.len(),
                            if snapshot.truncated { " (tail)" } else { "" }
                        ))
                        .size(11.0)
                        .color(colors.on_surface_variant),
                    );
                }
            });
        });

        ui.add_space(4.0);

        match &state.snapshot {
            Some(Ok(snapshot)) => {
                if state.filtered_indices.is_empty() {
                    let message = if state.filter.is_empty() {
                        empty_message
                    } else {
                        "No entries match the current filter"
                    };
                    Self::render_empty_file_log(
                        ui,
                        egui_phosphor::regular::FILE_TEXT,
                        message,
                        colors,
                    );
                } else {
                    render_file_lines(
                        ui,
                        scroll_id,
                        snapshot,
                        &state.filtered_indices,
                        state.auto_scroll,
                        colors,
                    );
                }
            }
            Some(Err(error)) => Self::render_error(ui, error, colors),
            None => Self::render_empty_file_log(
                ui,
                egui_phosphor::regular::FILE_TEXT,
                "No log loaded",
                colors,
            ),
        }

        refresh
    }

    fn render_error(ui: &mut Ui, error: &str, colors: &ThemeColors) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new(egui_phosphor::regular::WARNING)
                    .size(32.0)
                    .color(colors.error),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(error).size(12.0).color(colors.error));
        });
    }

    fn render_empty_file_log(ui: &mut Ui, icon: &str, message: &str, colors: &ThemeColors) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new(icon)
                    .size(40.0)
                    .color(colors.on_surface_variant),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(message).size(14.0).weak());
        });
    }
}

pub fn render_file_lines(
    ui: &mut Ui,
    scroll_id: &'static str,
    snapshot: &LogFileSnapshot,
    filtered_indices: &[usize],
    auto_scroll: bool,
    colors: &ThemeColors,
) {
    let mut scroll = egui::ScrollArea::both()
        .id_salt(scroll_id)
        .auto_shrink([false, false]);
    if auto_scroll {
        scroll = scroll.stick_to_bottom(true);
    }

    let previous_spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    scroll.show_rows(ui, LOG_ROW_HEIGHT, filtered_indices.len(), |ui, range| {
        for visible in range {
            render_log_row(ui, &snapshot.lines[filtered_indices[visible]], colors);
        }
    });
    ui.spacing_mut().item_spacing.y = previous_spacing;
}

fn render_log_row(ui: &mut Ui, line: &str, colors: &ThemeColors) {
    let (indicator_color, msg_color) = file_log_colors(line, colors);
    egui::Frame::NONE
        .fill(ui.style().visuals.faint_bg_color)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .corner_radius(2.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, indicator_color);
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(line).monospace().size(11.0).color(msg_color))
                        .extend(),
                );
            });
        });
}

fn source_label(source: &LogSource) -> String {
    match source {
        LogSource::CurrentSession => "Current session".to_string(),
        LogSource::File(path) => path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Selected log")
            .to_string(),
    }
}

fn file_log_colors(line: &str, colors: &ThemeColors) -> (egui::Color32, egui::Color32) {
    let lower = line.to_lowercase();
    if lower.contains(" error ") || lower.contains("error:") || lower.contains("failed") {
        (colors.error, colors.error)
    } else if lower.contains(" warn ") || lower.contains("warning") {
        (colors.warning, colors.on_surface)
    } else if lower.contains(" debug ") || lower.contains(" trace ") {
        (colors.on_surface_variant, colors.on_surface_variant)
    } else {
        (colors.primary, colors.on_surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn read_log_tail_returns_small_file_without_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arclain-2026-07-06.log");
        fs::write(&path, "first\nsecond\n").unwrap();

        let snapshot = read_log_tail(&path, 1024).unwrap();

        assert_eq!(snapshot.lines, vec!["first", "second"]);
        assert!(!snapshot.truncated);
        assert_eq!(snapshot.path, path);
    }

    #[test]
    fn read_log_tail_caps_large_file_at_line_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arclain-2026-07-06.log");
        fs::write(&path, "old line\nmiddle line\nnew line\n").unwrap();

        let snapshot = read_log_tail(&path, 22).unwrap();

        assert_eq!(snapshot.lines, vec!["middle line", "new line"]);
        assert!(snapshot.truncated);
    }

    #[test]
    fn read_log_tail_from_offset_returns_only_session_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arclain-2026-07-06.log");
        fs::write(&path, "before startup\n").unwrap();
        let offset = fs::metadata(&path).unwrap().len();
        fs::write(&path, "before startup\nafter startup\n").unwrap();

        let snapshot = read_log_tail_from_offset(&path, offset, 1024).unwrap();

        assert_eq!(snapshot.lines, vec!["after startup"]);
        assert!(!snapshot.truncated);
    }

    #[test]
    fn read_log_tail_from_offset_caps_large_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arclain-2026-07-06.log");
        fs::write(&path, "old\nsession one\nsession two\n").unwrap();
        let offset = "old\n".len() as u64;

        let snapshot = read_log_tail_from_offset(&path, offset, 14).unwrap();

        assert_eq!(snapshot.lines, vec!["session two"]);
        assert!(snapshot.truncated);
    }

    #[test]
    fn list_plugin_log_files_returns_only_logs_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("alpha-2026-07-05.log");
        let new = dir.path().join("beta-2026-07-06.log");
        let ignored = dir.path().join("notes.txt");
        fs::write(&old, "old").unwrap();
        thread::sleep(Duration::from_millis(50));
        fs::write(&new, "new").unwrap();
        fs::write(ignored, "ignore").unwrap();

        let files = list_plugin_log_files(dir.path()).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, new);
        assert_eq!(files[0].label, "beta-2026-07-06.log");
        assert_eq!(files[1].path, old);
    }

    #[test]
    fn read_plugin_session_logs_uses_offsets_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("dlsite-metadata-2026-07-06.log");
        let new = dir.path().join("new-plugin-2026-07-06.log");
        fs::write(&old, "old plugin line\n").unwrap();
        let session = LogSession::capture(dir.path().join("arclain-2026-07-06.log"), dir.path());
        fs::write(&old, "old plugin line\nnew plugin line\n").unwrap();
        fs::write(&new, "new file line\n").unwrap();

        let snapshot = read_plugin_session_logs(&session, 1024).unwrap();

        assert_eq!(
            snapshot.lines,
            vec![
                "[dlsite-metadata-2026-07-06.log] new plugin line",
                "[new-plugin-2026-07-06.log] new file line",
            ]
        );
    }

    #[test]
    fn tail_reader_never_exceeds_limit_if_file_grows_after_metadata() {
        let path = Path::new("growing.log");
        let mut contents = Cursor::new(b"old\nnew\nextra\n".to_vec());

        let (snapshot, bytes_read) =
            read_log_tail_from_reader(&mut contents, path, 4, 0, 4).unwrap();

        assert_eq!(bytes_read, 4);
        assert_eq!(contents.position(), 4);
        assert_eq!(snapshot.lines, ["old"]);
    }

    #[test]
    fn plugin_session_budget_counts_blank_and_partial_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let session = LogSession::capture(dir.path().join("arclain-2026-07-06.log"), dir.path());
        fs::write(dir.path().join("alpha.log"), "\n\n\n\n\n\n\nA\n").unwrap();
        fs::write(dir.path().join("beta.log"), "012345\nB\n").unwrap();

        let snapshot = read_plugin_session_logs(&session, 8).unwrap();

        assert_eq!(snapshot.lines, ["[alpha.log] A"]);
        assert!(snapshot.truncated);
    }

    fn disconnected_worker() -> LogWorker {
        let (job_sender, job_receiver) = mpsc::channel::<LogJob>();
        drop(job_receiver);
        let (_result_sender, result_receiver) = mpsc::channel();
        LogWorker {
            sender: job_sender,
            receiver: Mutex::new(result_receiver),
            next_id: AtomicU64::new(0),
        }
    }

    #[test]
    fn disconnected_worker_does_not_leave_catalog_requests_pending() {
        let mut state = LogsPageState::with_worker(
            LogSession::capture_with_app_offset("app.log".into(), 0, "plugins"),
            disconnected_worker(),
        );

        state.request_catalog_if_due(&egui::Context::default());

        assert!(state.app_list_pending.is_none());
        assert!(state.plugin_list_pending.is_none());
    }

    #[test]
    fn disconnected_worker_does_not_suppress_log_read_retry() {
        let worker = disconnected_worker();
        let mut view = FileLogViewState::new();
        let path = PathBuf::from("app.log");
        let key = LogReadKey::File(path.clone());
        let request = LogRequest::ReadFile {
            path,
            max_bytes: MAX_LOG_BYTES,
        };

        LogsPage::queue_log_read(
            &worker,
            &mut view,
            key.clone(),
            request.clone(),
            &egui::Context::default(),
            false,
        );
        LogsPage::queue_log_read(
            &worker,
            &mut view,
            key,
            request,
            &egui::Context::default(),
            false,
        );

        assert!(view.pending.is_none());
        assert_eq!(worker.next_id.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn stale_snapshot_result_cannot_replace_newer_source_request() {
        let mut view = FileLogViewState::new();
        let old_path = PathBuf::from("old.log");
        let new_path = PathBuf::from("new.log");
        view.begin_request(LogReadKey::File(old_path.clone()), LogRequestId(1));
        view.begin_request(LogReadKey::File(new_path.clone()), LogRequestId(2));

        assert!(!view.apply_snapshot(
            LogRequestId(1),
            Ok(LogFileSnapshot {
                path: old_path,
                lines: vec!["stale".to_string()],
                truncated: false,
            }),
        ));
        assert!(view.snapshot.is_none());

        assert!(view.apply_snapshot(
            LogRequestId(2),
            Ok(LogFileSnapshot {
                path: new_path,
                lines: vec!["current".to_string()],
                truncated: false,
            }),
        ));
        assert_eq!(
            view.snapshot.as_ref().unwrap().as_ref().unwrap().lines,
            ["current"]
        );
    }
}
