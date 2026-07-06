//! Diagnostics log page helpers and renderer.

use crate::shared::components::network_log::{NetworkLog, NetworkLogState};
use crate::shared::theme::ThemeColors;
use eframe::egui::{self, RichText, Ui};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const MAX_LOG_BYTES: u64 = 512 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

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
pub struct LogSession {
    pub app_log_path: PathBuf,
    pub app_log_offset: u64,
    pub plugin_log_dir: PathBuf,
    plugin_offsets: HashMap<PathBuf, u64>,
}

impl LogSession {
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

pub struct FileLogViewState {
    pub filter: String,
    pub auto_scroll: bool,
    snapshot: Option<Result<LogFileSnapshot, String>>,
    snapshot_key: Option<LogReadKey>,
    last_refresh: Option<Instant>,
}

impl FileLogViewState {
    fn new() -> Self {
        Self {
            filter: String::new(),
            auto_scroll: true,
            snapshot: None,
            snapshot_key: None,
            last_refresh: None,
        }
    }

    fn refresh_with<F>(&mut self, key: LogReadKey, read: F)
    where
        F: FnOnce() -> Result<LogFileSnapshot, String>,
    {
        self.snapshot = Some(read());
        self.snapshot_key = Some(key);
        self.last_refresh = Some(Instant::now());
    }

    fn maybe_refresh_with<F>(&mut self, key: LogReadKey, read: F)
    where
        F: FnOnce() -> Result<LogFileSnapshot, String>,
    {
        let key_changed = self.snapshot_key.as_ref() != Some(&key);
        let stale = self
            .last_refresh
            .map(|last| last.elapsed() >= REFRESH_INTERVAL)
            .unwrap_or(true);
        if key_changed || stale {
            self.refresh_with(key, read);
        }
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
}

impl LogsPageState {
    pub fn new() -> Self {
        let app_log_path = arclain_core::utilities::current_app_log_path();
        let plugin_log_dir = arclain_core::utilities::plugin_log_dir();
        Self::with_session(LogSession::capture(app_log_path, plugin_log_dir))
    }

    pub fn with_session(session: LogSession) -> Self {
        Self {
            active_tab: LogsTab::App,
            app_log: FileLogViewState::new(),
            plugin_log: FileLogViewState::new(),
            network_log: NetworkLogState::new(),
            session,
            app_source: LogSource::CurrentSession,
            plugin_source: LogSource::CurrentSession,
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
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let offset = if offset > len { 0 } else { offset };
    let max_bytes = max_bytes.max(1);
    let available = len.saturating_sub(offset);
    let truncated = available > max_bytes;
    let start = if truncated { len - max_bytes } else { offset };

    file.seek(SeekFrom::Start(start))?;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    if start > offset && !text.starts_with('\n') && !lines.is_empty() {
        lines.remove(0);
    }
    lines.retain(|line| !line.is_empty());

    Ok(LogFileSnapshot {
        path: path.to_path_buf(),
        lines,
        truncated,
    })
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
        let snapshot = read_log_tail_from_offset(&file.path, offset, remaining)?;
        let used = snapshot
            .lines
            .iter()
            .map(|line| line.len() as u64 + 1)
            .sum::<u64>();
        remaining = remaining.saturating_sub(used);
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
        let files = match list_app_log_files(&state.session.app_log_dir()) {
            Ok(files) => files,
            Err(error) => {
                Self::render_error(ui, &format!("Failed to list app logs: {}", error), colors);
                return;
            }
        };

        Self::render_source_combo(ui, "app_log_source", &mut state.app_source, &files);

        match state.app_source.clone() {
            LogSource::CurrentSession => {
                let key = LogReadKey::SessionFile(
                    state.session.app_log_path.clone(),
                    state.session.app_log_offset,
                );
                state.app_log.maybe_refresh_with(key.clone(), || {
                    read_log_tail_from_offset(
                        &state.session.app_log_path,
                        state.session.app_log_offset,
                        MAX_LOG_BYTES,
                    )
                    .map_err(|error| {
                        format!(
                            "Failed to read {}: {}",
                            state.session.app_log_path.display(),
                            error
                        )
                    })
                });
                let refresh = Self::render_file_log(
                    ui,
                    "app_log_scroll",
                    &mut state.app_log,
                    colors,
                    "No app log entries for this session",
                );
                if refresh {
                    state.app_log.refresh_with(key, || {
                        read_log_tail_from_offset(
                            &state.session.app_log_path,
                            state.session.app_log_offset,
                            MAX_LOG_BYTES,
                        )
                        .map_err(|error| {
                            format!(
                                "Failed to read {}: {}",
                                state.session.app_log_path.display(),
                                error
                            )
                        })
                    });
                }
            }
            LogSource::File(path) => {
                Self::render_selected_file_log(
                    ui,
                    "app_log_scroll",
                    &path,
                    &mut state.app_log,
                    colors,
                    "No entries in this app log",
                );
            }
        }
    }

    fn render_plugin_log(ui: &mut Ui, state: &mut LogsPageState, colors: &ThemeColors) {
        let files = match list_plugin_log_files(&state.session.plugin_log_dir) {
            Ok(files) => files,
            Err(error) => {
                Self::render_error(
                    ui,
                    &format!("Failed to list plugin logs: {}", error),
                    colors,
                );
                return;
            }
        };

        Self::render_source_combo(ui, "plugin_log_source", &mut state.plugin_source, &files);

        match state.plugin_source.clone() {
            LogSource::CurrentSession => {
                let key = LogReadKey::PluginSession;
                state.plugin_log.maybe_refresh_with(key.clone(), || {
                    read_plugin_session_logs(&state.session, MAX_LOG_BYTES).map_err(|error| {
                        format!(
                            "Failed to read plugin logs from {}: {}",
                            state.session.plugin_log_dir.display(),
                            error
                        )
                    })
                });
                let refresh = Self::render_file_log(
                    ui,
                    "plugin_log_scroll",
                    &mut state.plugin_log,
                    colors,
                    "No plugin log entries for this session",
                );
                if refresh {
                    state.plugin_log.refresh_with(key, || {
                        read_plugin_session_logs(&state.session, MAX_LOG_BYTES).map_err(|error| {
                            format!(
                                "Failed to read plugin logs from {}: {}",
                                state.session.plugin_log_dir.display(),
                                error
                            )
                        })
                    });
                }
            }
            LogSource::File(path) => {
                Self::render_selected_file_log(
                    ui,
                    "plugin_log_scroll",
                    &path,
                    &mut state.plugin_log,
                    colors,
                    "No entries in this plugin log",
                );
            }
        }
    }

    fn render_selected_file_log(
        ui: &mut Ui,
        scroll_id: &'static str,
        path: &Path,
        state: &mut FileLogViewState,
        colors: &ThemeColors,
        empty_message: &str,
    ) {
        let key = LogReadKey::File(path.to_path_buf());
        state.maybe_refresh_with(key.clone(), || {
            read_log_tail(path, MAX_LOG_BYTES)
                .map_err(|error| format!("Failed to read {}: {}", path.display(), error))
        });
        let refresh = Self::render_file_log(ui, scroll_id, state, colors, empty_message);
        if refresh {
            state.refresh_with(key, || {
                read_log_tail(path, MAX_LOG_BYTES)
                    .map_err(|error| format!("Failed to read {}: {}", path.display(), error))
            });
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
                            filtered_lines(snapshot, &state.filter).len(),
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
                let lines = filtered_lines(snapshot, &state.filter);
                if lines.is_empty() {
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
                    Self::render_file_lines(ui, scroll_id, &lines, state.auto_scroll, colors);
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

    fn render_file_lines(
        ui: &mut Ui,
        scroll_id: &'static str,
        lines: &[&str],
        auto_scroll: bool,
        colors: &ThemeColors,
    ) {
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .auto_shrink([false, false]);
        if auto_scroll {
            scroll = scroll.stick_to_bottom(true);
        }

        scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
            for line in lines {
                let (indicator_color, msg_color) = file_log_colors(line, colors);
                egui::Frame::NONE
                    .fill(ui.style().visuals.faint_bg_color)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(4.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, indicator_color);
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(*line).monospace().size(11.0).color(msg_color),
                                )
                                .wrap(),
                            );
                        });
                    });
            }
        });
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

fn filtered_lines<'a>(snapshot: &'a LogFileSnapshot, filter: &str) -> Vec<&'a str> {
    let filter = filter.to_lowercase();
    snapshot
        .lines
        .iter()
        .filter(|line| filter.is_empty() || line.to_lowercase().contains(&filter))
        .map(String::as_str)
        .collect()
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
}
