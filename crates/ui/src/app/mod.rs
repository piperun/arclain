pub mod archive_operations;
pub mod extraction_operations;
pub mod file_operations;
pub mod navigation;
pub mod navigation_operations;
pub mod state;
pub mod utils;
pub mod window_operations;

use crate::app::archive_operations::ArchiveInfo;
use crate::app::navigation::{AppPage, PageNavigator, SettingsPage};
use crate::app::state::AppState;
use crate::app::utils::{convert_to_file_entry, format_size};
use crate::features::{
    dialogs, file_list, header, load_cjk_fonts, plugins::types::PluginsListState,
    properties_panel, settings_content, settings_page, status_bar, toolbar, tree_panel, AppTheme,
};
use crate::platform::detect_dark_mode;

use arclain_core::file_opener::{FileOpener, OpenStrategy};
use arclain_core::sevenzip::ProgressUpdate;
use eframe::egui;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info};

pub struct ArclainApp {
    state: Arc<Mutex<AppState>>,
    theme: AppTheme,

    // Navigation
    page_navigator: PageNavigator,

    // UI State
    header_state: header::HeaderState,
    toolbar_state: toolbar::ToolbarState,
    sort_state: file_list::SortState,
    tree_state: tree_panel::TreePanelState,
    password_dialog: dialogs::PasswordDialog,
    edit_dialog: dialogs::FileEditDialog,
    password_rules_dialog: dialogs::PasswordRulesDialog,

    // Settings state
    security_settings_state: settings_content::SecuritySettingsState,
    plugins_state: PluginsListState,

    // Data
    entries: Vec<file_list::FileEntry>,
    status_info: status_bar::StatusBarInfo,
    current_path: String,
    pending_archive_path: Option<PathBuf>,
    pending_edit_file: Option<String>,
    pending_open_file: Option<String>,

    // Archive info
    archive_info: ArchiveInfo,
    last_window_title: Option<String>,

    // Extraction progress state
    extraction_dialog: dialogs::ExtractionProgressDialog,
    extraction_rx: Option<Receiver<ProgressUpdate>>,
    extraction_child: Option<std::process::Child>,
    extraction_minimized: bool,
    extraction_started: Option<Instant>,
    password_rules_loaded: bool,  // Track if we've loaded password rules for current settings session
}

impl ArclainApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);

        // Load CJK fonts during initialization to support Japanese/Chinese characters
        load_cjk_fonts(&cc.egui_ctx);

        let state = Arc::new(Mutex::new(
            AppState::new().expect("Failed to initialize app state"),
        ));

        Self {
            state,
            theme,
            page_navigator: PageNavigator::new(),
            header_state: header::HeaderState::default(),
            toolbar_state: toolbar::ToolbarState::default(),
            sort_state: file_list::SortState::default(),
            tree_state: tree_panel::TreePanelState::default(),
            password_dialog: dialogs::PasswordDialog::default(),
            edit_dialog: dialogs::FileEditDialog::default(),
            password_rules_dialog: dialogs::PasswordRulesDialog::default(),
            security_settings_state: settings_content::SecuritySettingsState::default(),
            plugins_state: PluginsListState::default(),
            entries: Vec::new(),
            status_info: status_bar::StatusBarInfo::default(),
            current_path: String::new(),
            pending_archive_path: None,
            pending_edit_file: None,
            pending_open_file: None,
            archive_info: ArchiveInfo::default(),
            last_window_title: None,
            extraction_dialog: dialogs::ExtractionProgressDialog::default(),
            extraction_rx: None,
            extraction_child: None,
            extraction_minimized: false,
            extraction_started: None,
            password_rules_loaded: false,
        }
    }
}

impl eframe::App for ArclainApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme
        self.theme.apply_to_context(ctx);

        // Safely set window title to opened archive name
        let desired_title = {
            let state = self.state.lock();
            if self.archive_info.archive_loaded {
                let base = state
                    .current_archive
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Archive");
                format!(
                    "{} - Arclain",
                    window_operations::sanitize_window_title(base)
                )
            } else {
                "Arclain".to_string()
            }
        };
        if self.last_window_title.as_deref() != Some(desired_title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(desired_title.clone()));
            self.last_window_title = Some(desired_title);
        }

        // Header
        egui::TopBottomPanel::top("header")
            .exact_height(52.0)
            .frame(
                egui::Frame::NONE
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)),
            )
            .show(ctx, |ui| {
                let mut toggle_theme = false;
                let show_nav_buttons = !self.page_navigator.is_on_main();
                let can_go_back = self.page_navigator.can_go_back();

                let header_actions = header::render(
                    ui,
                    &self.theme,
                    &mut self.header_state,
                    &mut toggle_theme,
                    show_nav_buttons,
                    can_go_back,
                );

                if toggle_theme {
                    self.theme.toggle();
                }

                // Handle navigation from header
                if header_actions.navigate_home {
                    self.page_navigator.navigate_to_main();
                }
                if header_actions.navigate_back {
                    self.page_navigator.navigate_back();
                }
                if header_actions.navigate_settings {
                    self.page_navigator
                        .navigate_to(AppPage::Settings(SettingsPage::Overview));
                    // Prefill security settings from current state
                    let st = self.state.lock();
                    if let Some(paths) = &st.db_paths {
                        self.security_settings_state.key_file_path = paths
                            .key_file
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.security_settings_state.secrets_db_path =
                            paths.secrets_db.to_string_lossy().to_string();
                    } else {
                        self.security_settings_state.key_file_path.clear();
                        self.security_settings_state.secrets_db_path.clear();
                    }
                    self.security_settings_state.encrypted_crc_policy =
                        match st.encrypted_crc_policy.as_str() {
                            "prompt_on_open" => dialogs::EncryptedCrcPolicy::PromptOnOpen,
                            "on_access" => dialogs::EncryptedCrcPolicy::OnAccess,
                            _ => dialogs::EncryptedCrcPolicy::OnOpen,
                        };
                }
            });

        // Toolbar
        egui::TopBottomPanel::top("toolbar")
            .exact_height(52.0)
            .frame(
                egui::Frame::NONE
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)),
            )
            .show(ctx, |ui| {
                let state = self.state.lock();
                let can_go_back = state.navigation.can_go_back();
                let can_go_forward = state.navigation.can_go_forward();
                let can_go_up = state.navigation.can_go_up();
                drop(state);

                let has_selection = self.entries.iter().any(|e| e.selected);
                let actions = toolbar::render(
                    ui,
                    &self.theme,
                    &mut self.toolbar_state,
                    can_go_back,
                    can_go_forward,
                    can_go_up,
                    self.archive_info.archive_loaded,
                    has_selection,
                );

                if actions.open {
                    archive_operations::open_archive(
                        &self.state,
                        &mut self.current_path,
                        &mut self.password_dialog,
                        &mut self.pending_archive_path,
                        &mut self.status_info,
                        &mut self.entries,
                        &mut self.archive_info,
                    );
                }
                if actions.go_back {
                    navigation_operations::navigate_back(
                        &self.state,
                        &mut self.entries,
                        &mut self.current_path,
                    );
                }
                if actions.go_forward {
                    navigation_operations::navigate_forward(
                        &self.state,
                        &mut self.entries,
                        &mut self.current_path,
                    );
                }
                if actions.go_up {
                    navigation_operations::navigate_up(
                        &self.state,
                        &mut self.entries,
                        &mut self.current_path,
                    );
                }
                if actions.extract {
                    if self.extraction_child.is_none() {
                        extraction_operations::extract_selected(
                            &self.state,
                            &self.entries,
                            &None,
                            &mut self.extraction_dialog,
                            &mut self.extraction_rx,
                            &mut self.extraction_child,
                            &mut self.extraction_minimized,
                            &mut self.extraction_started,
                            &mut self.status_info,
                        );
                    } else {
                        self.status_info.message =
                            "Another extraction is already running".to_string();
                    }
                }
                if actions.extract_all {
                    if self.extraction_child.is_none() {
                        extraction_operations::extract_all(
                            &self.state,
                            &None,
                            &mut self.extraction_dialog,
                            &mut self.extraction_rx,
                            &mut self.extraction_child,
                            &mut self.extraction_minimized,
                            &mut self.extraction_started,
                            &mut self.status_info,
                        );
                    } else {
                        self.status_info.message =
                            "Another extraction is already running".to_string();
                    }
                }
                if actions.add {
                    file_operations::add_files(&self.state, &mut self.status_info);
                }
                if actions.delete_selected {
                    // Clone entries for read, then update
                    let entries_clone = self.entries.clone();
                    file_operations::delete_selected(
                        &self.state,
                        &entries_clone,
                        &mut self.status_info,
                        &mut self.entries,
                        &mut self.archive_info,
                    );
                }
            });

        // Status bar
        egui::TopBottomPanel::bottom("status")
            .exact_height(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(0, 8)),
            )
            .show(ctx, |ui| {
                // Collect plugin status info
                let plugin_status = {
                    let st = self.state.lock();
                    if let Some(ref manager_arc) = st.plugin_manager {
                        let manager = manager_arc.lock();
                        let plugins = manager.list_plugins();
                        let enabled_count = plugins.iter().filter(|p| p.enabled).count();
                        Some(status_bar::PluginStatusInfo {
                            total_plugins: plugins.len(),
                            enabled_plugins: enabled_count,
                            has_metadata: st.plugin_metadata.is_some(),
                        })
                    } else {
                        None
                    }
                };

                // Left area: main status
                status_bar::render(
                    ui,
                    &self.theme,
                    &self.status_info,
                    self.archive_info.archive_loaded,
                    plugin_status.as_ref(),
                );

                // Right-aligned: background progress chip when minimized
                if self.extraction_minimized {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.extraction_dialog.percent >= 100 {
                            "Extraction done".to_string()
                        } else {
                            format!("Extracting… {}%", self.extraction_dialog.percent)
                        };
                        let resp = status_bar::progress_chip(ui, &self.theme, &label);
                        if resp.clicked() {
                            self.extraction_dialog.show = true;
                            self.extraction_minimized = false;
                        }
                    });
                }
            });

        // Render based on current page
        match &self.page_navigator.current_page {
            AppPage::Main => {
                self.render_main_page(ctx);
            }
            AppPage::Settings(settings_page) => {
                self.render_settings_page(ctx, settings_page.clone());
            }
        }

        // Dialogs (shown on top of everything)
        self.render_dialogs(ctx);

        // Background pump for extraction progress
        if let Some(rx) = &self.extraction_rx {
            for upd in rx.try_iter() {
                if upd.percent > 0 {
                    self.extraction_dialog.percent = upd.percent;
                }
                if let Some(msg) = upd.message {
                    // Keep last ~500 lines
                    if self.extraction_dialog.log_lines.len() > 500 {
                        let overflow = self.extraction_dialog.log_lines.len() - 500;
                        self.extraction_dialog.log_lines.drain(0..overflow);
                    }
                    self.extraction_dialog.log_lines.push(msg);
                }
                if let Some(start) = self.extraction_started {
                    let elapsed = start.elapsed();
                    self.extraction_dialog.elapsed_text =
                        window_operations::format_duration(elapsed);
                    if upd.percent > 0 && upd.percent < 100 {
                        let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                        let left = total_est.saturating_sub(elapsed);
                        self.extraction_dialog.time_left_text =
                            window_operations::format_duration(left);
                        self.extraction_dialog.processed_text = format!("{}%", upd.percent);
                    }
                }
                ctx.request_repaint();
            }
        }

        // Check child completion
        if let Some(child) = self.extraction_child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                if status.success() && self.extraction_dialog.percent >= 100 {
                    self.extraction_dialog.status = dialogs::ExtractionStatus::Completed;
                    self.status_info.message = "Extraction completed".to_string();

                    // If we were opening a file, do it now
                    if let Some(full_path) = self.pending_open_file.take() {
                        info!("Extraction complete, opening file: {}", full_path);
                        let temp_base =
                            std::env::temp_dir().join(format!("arclain_{}", std::process::id()));
                        let normalized_full_path =
                            full_path.replace('/', std::path::MAIN_SEPARATOR.to_string().as_str());
                        let file_to_open = temp_base.join(&normalized_full_path);

                        if file_to_open.exists() {
                            match open::that(&file_to_open) {
                                Ok(()) => {
                                    let file_name =
                                        full_path.split('/').last().unwrap_or(&full_path);
                                    self.status_info.message = format!("Opened {}", file_name);
                                }
                                Err(e) => {
                                    error!("Failed to open file: {}", e);
                                    self.status_info.message =
                                        format!("Failed to open file: {}", e);
                                }
                            }
                        } else {
                            error!("Extracted file not found: {}", file_to_open.display());
                            self.status_info.message =
                                "File not found after extraction".to_string();
                        }
                    }
                } else {
                    self.extraction_dialog.status = dialogs::ExtractionStatus::Failed;
                    self.status_info.message =
                        format!("Extraction ended with status: {:?}", status.code());
                }
                // Auto-hide when completed unless minimized
                if !self.extraction_minimized {
                    self.extraction_dialog.show = false;
                }
                self.extraction_child = None;
                self.extraction_rx = None;
                self.extraction_started = None;
            }
        }
    }
}

impl ArclainApp {
    /// Render the main archive viewer page
    fn render_main_page(&mut self, ctx: &egui::Context) {
        // Left panel - Tree view
        if self.toolbar_state.show_tree_panel && self.archive_info.archive_loaded {
            egui::SidePanel::left("tree_panel")
                .exact_width(240.0)
                .frame(egui::Frame::NONE.fill(self.theme.colors.bg_secondary))
                .show(ctx, |ui| {
                    let state = self.state.lock();
                    let archive_name = state
                        .current_archive
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "archive".to_string());

                    let folders = state.navigation.get_all_folders(&state.all_entries);
                    let current_path = state.navigation.current_path.clone();
                    drop(state);

                    if let Some(path) = tree_panel::render(
                        ui,
                        &self.theme,
                        &mut self.tree_state,
                        &archive_name,
                        &folders,
                        &current_path,
                    ) {
                        if path.is_empty() {
                            // Navigate to root
                            let mut state = self.state.lock();
                            state.navigation.current_path.clear();
                            state.navigation.path_stack.clear();
                            state.navigation.forward_stack.clear();
                            self.entries = state
                                .get_current_entries()
                                .iter()
                                .map(convert_to_file_entry)
                                .collect();
                            let current_archive = state.current_archive.clone();
                            drop(state);
                            navigation_operations::update_current_path(
                                &mut self.current_path,
                                String::new(),
                                current_archive,
                            );
                        } else {
                            // Direct navigation to a specific path (not relative)
                            let mut state = self.state.lock();
                            state.navigation.set_current_path(&path);
                            state.navigation.forward_stack.clear();
                            self.entries = state
                                .get_current_entries()
                                .iter()
                                .map(convert_to_file_entry)
                                .collect();
                            let current_archive = state.current_archive.clone();
                            drop(state);
                            navigation_operations::update_current_path(
                                &mut self.current_path,
                                path,
                                current_archive,
                            );
                        }
                    }
                });
        }

        // Right panel - Properties
        if self.toolbar_state.show_properties_panel && self.archive_info.archive_loaded {
            egui::SidePanel::right("properties_panel")
                .exact_width(280.0)
                .frame(
                    egui::Frame::NONE
                        .fill(self.theme.colors.bg_secondary)
                        .inner_margin(egui::Margin::symmetric(16, 16)),
                )
                .show(ctx, |ui| {
                    let mut groups = vec![properties_panel::create_archive_info_group(
                        &self.archive_info.archive_format,
                        self.archive_info.file_count,
                        &format_size(self.archive_info.total_size),
                        &format_size(self.archive_info.compressed_size),
                        self.archive_info.total_crc32.as_deref(),
                        self.archive_info.archive_encrypted,
                        self.archive_info.headers_encrypted,
                        self.archive_info.encryption_method.as_deref(),
                    )];

                    // Add plugin metadata if available
                    if let Some(metadata) = &self.state.lock().plugin_metadata {
                        if let Some(plugin_group) = properties_panel::create_plugin_metadata_group(metadata) {
                            groups.push(plugin_group);
                        }
                    }

                    properties_panel::render(ui, &self.theme, &groups);
                });
        }

        // Central panel - File list
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(self.theme.colors.bg_primary))
            .show(ctx, |ui| {
                if !self.archive_info.archive_loaded {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("📦").size(64.0));
                            ui.add_space(16.0);
                            ui.label(
                                egui::RichText::new("No archive loaded")
                                    .size(18.0)
                                    .color(self.theme.colors.text_primary),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Click 'Open' to load an archive")
                                    .size(14.0)
                                    .color(self.theme.colors.text_secondary),
                            );
                        });
                    });
                } else {
                    let mut breadcrumb_nav: Option<String> = None;

                    ui.vertical(|ui| {
                        // Breadcrumb
                        egui::Frame::NONE
                            .fill(self.theme.colors.bg_secondary)
                            .inner_margin(egui::Margin::symmetric(16, 10))
                            .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color))
                            .show(ui, |ui| {
                                let state = self.state.lock();
                                let archive_name = state
                                    .current_archive
                                    .as_ref()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let current_path = state.navigation.current_path.clone();
                                drop(state);

                                breadcrumb_nav = file_list::render_breadcrumb(
                                    ui,
                                    &self.theme,
                                    &current_path,
                                    &archive_name,
                                );
                            });

                        // File list scroll area
                        egui::ScrollArea::vertical()
                            .id_salt("file_list_scroll")
                            .show(ui, |ui| {
                                if self.toolbar_state.grid_view {
                                    if let Some(action) = file_list::render_grid_view(ui, &self.theme, &mut self.entries) {
                                        match action {
                                            file_list::FileListAction::Navigate(folder) => {
                                                navigation_operations::navigate_to(&self.state, &folder, &mut self.entries, &mut self.current_path);
                                            }
                                            file_list::FileListAction::Open(name) => {
                                                info!("[GRID VIEW] Open action triggered for: {}", name);
                                                // Build full path within archive
                                                let full_path = {
                                                    let st = self.state.lock();
                                                    let prefix = st.navigation.current_path.clone();
                                                    if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) }
                                                };
                                                // Pre-check encryption and prompt before attempting extraction
                                                let need_pw = {
                                                    let st = self.state.lock();
                                                    let is_encrypted = st.all_entries.iter().any(|e| e.path == full_path && e.encrypted);
                                                    let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(archive_name, &st.last_entries).is_some();
                                                    is_encrypted && !have_pw
                                                };
                                                if need_pw {
                                                    self.password_dialog.show = true;
                                                    self.password_dialog.password.clear();
                                                    self.password_dialog.error.clear();
                                                    self.pending_archive_path = { let st = self.state.lock(); st.current_archive.clone() };
                                                    self.pending_open_file = Some(full_path.clone());
                                                    self.status_info.message = "Password required to open file".to_string();
                                                    return;
                                                }
                                                let tmp_dir = match tempfile::tempdir() { Ok(d) => d, Err(e) => { self.status_info.message = format!("Open failed: {}", e); return; } };
                                                let dest_path = tmp_dir.path().to_path_buf();
                                                let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                                                if let Some(archive) = archive_opt {
                                                    let res = { self.state.lock().extract_specific(&archive, &dest_path, vec![full_path.clone()]) };
                                                    match res {
                                                        Ok(()) => {
                                                            let extracted = dest_path.join(&name);
                                                            if let Err(e) = open::that(&extracted) {
                                                                self.status_info.message = format!("Failed to open file: {}", e);
                                                            }
                                                            std::mem::forget(tmp_dir);
                                                        }
                                                        Err(e) => {
                                                            let err_msg = e.to_string();
                                                            if err_msg.contains("Wrong password")
                                                                || err_msg.contains("Cannot open encrypted")
                                                                || err_msg.contains("code Some(2)") {
                                                                self.password_dialog.show = true; self.password_dialog.password.clear(); self.password_dialog.error.clear();
                                                                self.pending_archive_path = { let st = self.state.lock(); st.current_archive.clone() };
                                                                self.pending_open_file = Some(full_path.clone());
                                                                self.status_info.message = "Password required to open file".to_string();
                                                            } else { self.status_info.message = format!("Open failed: {}", err_msg); }
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                } else {
                                    if let Some(action) = file_list::render_list_view(
                                        ui,
                                        &self.theme,
                                        &mut self.entries,
                                        self.toolbar_state.columns_locked,
                                        &mut self.sort_state,
                                    ) {
                                        match action {
                                            file_list::FileListAction::Navigate(folder) => {
                                                navigation_operations::navigate_to(&self.state, &folder, &mut self.entries, &mut self.current_path);
                                            }
                                            file_list::FileListAction::Edit(name) => {
                                                // Build full path in archive
                                                let full_path = {
                                                    let st = self.state.lock();
                                                    let prefix = st.navigation.current_path.clone();
                                                    if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) }
                                                };

                                                // Pre-check encryption and prompt before reading encrypted file
                                                let need_pw = {
                                                    let st = self.state.lock();
                                                    let is_encrypted = st.all_entries.iter().any(|e| e.path == full_path && e.encrypted);
                                                    let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(archive_name, &st.last_entries).is_some();
                                                    is_encrypted && !have_pw
                                                };
                                                if need_pw {
                                                    self.password_dialog.show = true;
                                                    self.password_dialog.password.clear();
                                                    self.password_dialog.error.clear();
                                                    self.pending_archive_path = { let st = self.state.lock(); st.current_archive.clone() };
                                                    self.pending_edit_file = Some(full_path.clone());
                                                    self.status_info.message = "File is encrypted - password required".to_string();
                                                    return;
                                                }

                                                info!("Edit action triggered for file: {}", name);

                                                // Check if file is likely a text file based on extension
                                                let is_text_file = full_path.to_lowercase().ends_with(".txt")
                                                    || full_path.to_lowercase().ends_with(".md")
                                                    || full_path.to_lowercase().ends_with(".json")
                                                    || full_path.to_lowercase().ends_with(".xml")
                                                    || full_path.to_lowercase().ends_with(".html")
                                                    || full_path.to_lowercase().ends_with(".css")
                                                    || full_path.to_lowercase().ends_with(".js")
                                                    || full_path.to_lowercase().ends_with(".log")
                                                    || full_path.to_lowercase().ends_with(".cfg")
                                                    || full_path.to_lowercase().ends_with(".ini")
                                                    || full_path.to_lowercase().ends_with(".conf")
                                                    || full_path.to_lowercase().ends_with(".py")
                                                    || full_path.to_lowercase().ends_with(".rs")
                                                    || full_path.to_lowercase().ends_with(".c")
                                                    || full_path.to_lowercase().ends_with(".cpp")
                                                    || full_path.to_lowercase().ends_with(".h")
                                                    || full_path.to_lowercase().ends_with(".hpp");

                                                // Load content (or placeholder for binary files)
                                                let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                                                if let Some(archive) = archive_opt {
                                                    let content = if is_text_file {
                                                        match self.state.lock().read_text_file(&archive, &full_path) {
                                                            Ok(text) => text,
                                                            Err(e) => {
                                                                let err_msg = e.to_string();
                                                                // Check if it's a password error
                                                                if err_msg.contains("Wrong password") || err_msg.contains("Cannot open encrypted") {
                                                                    error!("Password required to read file: {}", err_msg);
                                                                    // Show password dialog and remember which file to edit
                                                                    self.password_dialog.show = true;
                                                                    self.password_dialog.password.clear();
                                                                    self.password_dialog.error.clear();
                                                                    self.pending_archive_path = Some(archive.clone());
                                                                    self.pending_edit_file = Some(full_path.clone());
                                                                    self.status_info.message = "File is encrypted - password required".to_string();
                                                                    return; // Don't open edit dialog yet
                                                                } else {
                                                                    error!("Failed to read file content: {}", err_msg);
                                                                    self.status_info.message = format!("Failed to open for edit: {}", err_msg);
                                                                    String::new() // Continue with empty content on other errors
                                                                }
                                                            }
                                                        }
                                                    } else {
                                                        "[Binary file - content editing not supported]\n\nYou can still rename this file using the filename field above.".to_string()
                                                    };

                                                    // Show edit dialog
                                                    self.edit_dialog.show = true;
                                                    self.edit_dialog.full_path_in_archive = full_path.clone();
                                                    self.edit_dialog.name_input = name.clone();
                                                    self.edit_dialog.content = content;
                                                    self.edit_dialog.error.clear();
                                                }
                                            }
                                            file_list::FileListAction::Open(name) => {
                                                info!("[LIST VIEW] Open action triggered for: {}", name);
                                                // Build full path within archive
                                                let full_path = {
                                                    let st = self.state.lock();
                                                    let prefix = st.navigation.current_path.clone();
                                                    if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) }
                                                };

                                                // Pre-check encryption and prompt before attempting extraction
                                                let need_pw = {
                                                    let st = self.state.lock();
                                                    let is_encrypted = st.all_entries.iter().any(|e| e.path == full_path && e.encrypted);
                                                    let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(archive_name, &st.last_entries).is_some();
                                                    is_encrypted && !have_pw
                                                };
                                                if need_pw {
                                                    self.password_dialog.show = true;
                                                    self.password_dialog.password.clear();
                                                    self.password_dialog.error.clear();
                                                    self.pending_archive_path = { let st = self.state.lock(); st.current_archive.clone() };
                                                    self.pending_open_file = Some(full_path.clone());
                                                    self.status_info.message = "Password required to open file".to_string();
                                                    return;
                                                }

                                                // Create FileOpener for smart extraction
                                                let opener = match FileOpener::new() {
                                                    Ok(o) => o,
                                                    Err(e) => {
                                                        error!("Failed to create FileOpener: {}", e);
                                                        self.status_info.message = format!("Open failed: {}", e);
                                                        return;
                                                    }
                                                };

                                                // Determine which files to extract (same directory strategy)
                                                let all_entry_paths: Vec<String> = {
                                                    let st = self.state.lock();
                                                    st.all_entries.iter().map(|e| e.path.clone()).collect()
                                                };

                                                // Compute related files and ensure the target file is first.
                                                let mut files_to_extract = opener.get_files_to_extract(
                                                    &full_path,
                                                    &all_entry_paths,
                                                    OpenStrategy::SameDirectory,
                                                );
                                                if files_to_extract.first().map(|p| p != &full_path).unwrap_or(true) {
                                                    if let Some(pos) = files_to_extract.iter().position(|p| p == &full_path) {
                                                        let item = files_to_extract.remove(pos);
                                                        files_to_extract.insert(0, item);
                                                    } else {
                                                        // Ensure the clicked file is included
                                                        files_to_extract.insert(0, full_path.clone());
                                                    }
                                                }

                                                info!("Opening file: {} (extracting {} related files)", name, files_to_extract.len());

                                                // Check if extraction is already running
                                                if self.extraction_child.is_some() {
                                                    self.status_info.message = "Another extraction is already running".to_string();
                                                    return;
                                                }

                                                // Extract files with progress dialog
                                                let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                                                if let Some(archive) = archive_opt {
                                                    let backend = { let st = self.state.lock(); st.backend.clone() };
                                                    let auto_pw = { 
                                                        let st = self.state.lock(); 
                                                        let archive_name = st.current_archive.as_ref().and_then(|p| p.to_str());
                                                        st.cfg.auto_password_for(archive_name, &st.last_entries)
                                                    };
                                                    let pw_opt = { let st = self.state.lock(); st.current_password.as_deref().or(auto_pw.as_deref()).map(|s| s.to_string()) };
                                                    
                                                    match backend.spawn_extract_files_with_progress(&archive, opener.temp_dir(), &files_to_extract, pw_opt.as_deref()) {
                                                        Ok(handle) => {
                                                            self.extraction_dialog = dialogs::ExtractionProgressDialog::default();
                                                            self.extraction_dialog.show = true;
                                                            self.extraction_dialog.title = format!("Opening {}", name);
                                                            self.extraction_dialog.file_action = format!("Extracting {} related files", files_to_extract.len());
                                                            #[cfg(target_os = "windows")] { self.extraction_dialog.can_pause = true; }
                                                            self.extraction_rx = Some(handle.rx);
                                                            self.extraction_child = Some(handle.child);
                                                            self.extraction_minimized = false;
                                                            self.extraction_started = Some(Instant::now());
                                                            self.pending_open_file = Some(full_path.clone());
                                                            self.status_info.message = "Extracting files...".to_string();
                                                            std::mem::forget(opener);
                                                        }
                                                        Err(e) => {
                                                            self.status_info.message = format!("Failed to start extraction: {}", e);
                                                        }
                                                    }
                                                }
                                            }
                                            file_list::FileListAction::Delete(name) => {
                                                let full_path = {
                                                    let st = self.state.lock();
                                                    let prefix = st.navigation.current_path.clone();
                                                    if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) }
                                                };
                                                let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                                                if let Some(archive) = archive_opt {
                                                    let del_res = { self.state.lock().delete_files(&archive, &[full_path.clone()]) };
                                                    if let Err(e) = del_res {
                                                        self.status_info.message = format!("Delete failed: {}", e);
                                                    } else {
                                                        // Refresh listing
                                                        let mut st = self.state.lock();
                                                        if let Some(a) = st.current_archive.clone() {
                                                            if let Ok(entries) = st.list_archive(&a) {
                                                                let current_archive = st.current_archive.clone();
                                                                drop(st);
                                                                archive_operations::load_archive_data(
                                                                    &self.state,
                                                                    entries,
                                                                    current_archive,
                                                                    &mut self.password_dialog,
                                                                    &mut self.pending_archive_path,
                                                                    &mut self.status_info,
                                                                    &mut self.entries,
                                                                    &mut self.archive_info,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            });
                    });

                    // Handle breadcrumb navigation
                    if let Some(path) = breadcrumb_nav {
                        // Direct navigation to the clicked path
                        let mut state = self.state.lock();
                        state.navigation.set_current_path(&path);
                        state.navigation.forward_stack.clear();
                        self.entries = state
                            .get_current_entries()
                            .iter()
                            .map(convert_to_file_entry)
                            .collect();
                        let current_archive = state.current_archive.clone();
                        drop(state);
                        navigation_operations::update_current_path(&mut self.current_path, path, current_archive);
                    }
                }
            });
    }

    /// Render the settings page
    fn render_settings_page(&mut self, ctx: &egui::Context, settings_page: SettingsPage) {
        // Load password rules when entering Password Rules page (only once per session)
        if matches!(settings_page, SettingsPage::PasswordRules) && !self.password_rules_loaded {
            let st = self.state.lock();
            self.password_rules_dialog.rules = st
                .cfg
                .cfg
                .pass_rules
                .iter()
                .map(|r| dialogs::PasswordRule {
                    name: r.name.clone(),
                    pattern: r.pattern.clone(),
                    password: r.password.clone(),
                    priority: r.priority,
                    enabled: r.enabled,
                })
                .collect();
            self.password_rules_loaded = true;
        }
        // Reset flag when leaving password rules page
        if !matches!(settings_page, SettingsPage::PasswordRules) {
            self.password_rules_loaded = false;
        }

        // Left panel - Settings navigator
        egui::SidePanel::left("settings_navigator")
            .exact_width(240.0)
            .frame(egui::Frame::NONE.fill(self.theme.colors.bg_secondary))
            .show(ctx, |ui| {
                if let Some(selected) =
                    settings_page::render_settings_navigator(ui, &self.theme, &settings_page)
                {
                    self.page_navigator.navigate_to(AppPage::Settings(selected));
                }
            });

        // Central panel - Settings content
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(self.theme.colors.bg_primary))
            .show(ctx, |ui| {
                ui.add_space(16.0);

                match &settings_page {
                    SettingsPage::Overview => {
                        // Show settings overview with category cards
                        if let Some(selected) =
                            settings_page::render_settings_overview(ui, &self.theme)
                        {
                            self.page_navigator.navigate_to(AppPage::Settings(selected));
                        }
                    }
                    _ => {
                        // Show category header with back button
                        let mut should_go_back = false;
                        egui::Frame::NONE
                            .fill(self.theme.colors.bg_secondary)
                            .inner_margin(egui::Margin::symmetric(20, 16))
                            .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color))
                            .show(ui, |ui| {
                                settings_page::render_settings_header(
                                    ui,
                                    &self.theme,
                                    &settings_page,
                                    &mut should_go_back,
                                );
                            });

                        if should_go_back {
                            self.page_navigator.navigate_back();
                        }

                        ui.add_space(16.0);

                        // Render settings content
                        egui::ScrollArea::vertical()
                            .id_salt("settings_content_scroll")
                            .show(ui, |ui| {
                                ui.add_space(8.0);

                                // Get plugin manager reference for settings
                                let st = self.state.lock();
                                let plugin_manager_ref = st.plugin_manager.as_ref();
                                
                                let action = if let Some(pm_mutex) = plugin_manager_ref {
                                    let pm = pm_mutex.lock();
                                    settings_content::render_settings_content(
                                        ui,
                                        &self.theme,
                                        &settings_page,
                                        &mut self.security_settings_state,
                                        &mut self.password_rules_dialog,
                                        Some(&pm),
                                        &mut self.plugins_state,
                                    )
                                } else {
                                    settings_content::render_settings_content(
                                        ui,
                                        &self.theme,
                                        &settings_page,
                                        &mut self.security_settings_state,
                                        &mut self.password_rules_dialog,
                                        None,
                                        &mut self.plugins_state,
                                    )
                                };
                                drop(st);

                                // Handle settings actions
                                if let Some(action) = action {
                                    self.handle_settings_action(action);
                                }

                                // For password rules page, save changes when modified
                                if matches!(settings_page, SettingsPage::PasswordRules) {
                                    // Check if we need to save (rules have been modified in the UI)
                                    // This will be handled by adding a save button or auto-save mechanism
                                }
                            });
                    }
                }
            });
    }

    /// Handle actions from settings pages
    fn handle_settings_action(&mut self, action: settings_content::SettingsAction) {
        match action {
            settings_content::SettingsAction::InstallPlugin { wasm_path } => {
                info!("Installing plugin from: {}", wasm_path);
                let st = self.state.lock();
                if let Some(ref manager_arc) = st.plugin_manager {
                    let mut manager = manager_arc.lock();
                    match manager.install_plugin(std::path::Path::new(&wasm_path)) {
                        Ok(plugin_id) => {
                            info!("Plugin installed successfully: {}", plugin_id);
                            self.status_info.message = format!("Plugin '{}' installed successfully", plugin_id);
                            // Update plugins state
                            self.plugins_state.update_from_manager(&manager);
                        }
                        Err(e) => {
                            error!("Failed to install plugin: {}", e);
                            self.status_info.message = format!("Failed to install plugin: {}", e);
                        }
                    }
                } else {
                    self.status_info.message = "Plugin system not available".to_string();
                }
            }
            settings_content::SettingsAction::SavePasswordRules { rules } => {
                // Convert UI rules to core PassRule format
                let pass_rules: Vec<arclain_core::PassRule> = rules
                    .iter()
                    .map(|r| arclain_core::PassRule {
                        name: r.name.clone(),
                        pattern: r.pattern.clone(),
                        password: r.password.clone(),
                        priority: r.priority,
                        enabled: r.enabled,
                    })
                    .collect();

                // Save to database via state
                let res = { self.state.lock().save_password_rules(pass_rules.clone()) };
                match res {
                    Ok(()) => {
                        self.password_rules_dialog.error.clear();
                        self.status_info.message = "Password rules saved successfully".to_string();
                        // Update the in-memory config so next load gets the saved rules
                        {
                            let mut st = self.state.lock();
                            st.cfg.cfg.pass_rules = pass_rules;
                        }
                    }
                    Err(e) => {
                        error!("Failed to save password rules: {}", e);
                        self.password_rules_dialog.error = format!("Failed to save: {}", e);
                        self.status_info.message = format!("Failed to save password rules: {}", e);
                    }
                }
            }
            settings_content::SettingsAction::SaveSecurity {
                key_file_path,
                secrets_db_path,
                encrypted_crc_policy,
            } => {
                let res = self.state.lock().apply_preferences(
                    key_file_path,
                    secrets_db_path,
                    encrypted_crc_policy,
                );
                match res {
                    Ok(()) => {
                        self.security_settings_state.error.clear();
                        self.security_settings_state.info =
                            "Settings saved successfully".to_string();
                        self.status_info.message = "Security settings saved".to_string();
                    }
                    Err(e) => {
                        error!("Failed to save security settings: {}", e);
                        self.security_settings_state.error = format!("Failed to save: {}", e);
                    }
                }
            }
            settings_content::SettingsAction::MoveVault { dest_path } => {
                let res = self.state.lock().move_vault(&dest_path);
                match res {
                    Ok(()) => {
                        self.security_settings_state.error.clear();
                        self.security_settings_state.info = format!("Vault moved to {}", dest_path);
                        self.status_info.message = "Vault moved successfully".to_string();
                    }
                    Err(e) => {
                        error!("Failed to move vault: {}", e);
                        self.security_settings_state.error = format!("Failed to move vault: {}", e);
                    }
                }
            }
            settings_content::SettingsAction::RekeyVault { new_key_file_path } => {
                let res = self.state.lock().rekey_vault(&new_key_file_path);
                match res {
                    Ok(()) => {
                        self.security_settings_state.error.clear();
                        self.security_settings_state.info =
                            "Vault rekeyed successfully".to_string();
                        self.status_info.message = "Vault rekeyed".to_string();
                    }
                    Err(e) => {
                        error!("Failed to rekey vault: {}", e);
                        self.security_settings_state.error =
                            format!("Failed to rekey vault: {}", e);
                    }
                }
            }
        }
    }

    /// Render all dialogs
    fn render_dialogs(&mut self, ctx: &egui::Context) {
        // Extraction progress dialog
        if self.extraction_dialog.show {
            if let Some(result) = dialogs::render_extraction_progress_dialog(
                ctx,
                &self.theme,
                &mut self.extraction_dialog,
            ) {
                match result {
                    dialogs::ExtractionDialogResult::Minimized => {
                        self.extraction_minimized = true;
                        self.extraction_dialog.show = false;
                        self.status_info.message = "Extraction running in background".to_string();
                    }
                    dialogs::ExtractionDialogResult::Paused => {
                        if let Some(child) = &self.extraction_child {
                            #[cfg(target_os = "windows")]
                            {
                                let _ = crate::platform::suspend_process(child.id());
                            }
                            self.extraction_dialog.status = dialogs::ExtractionStatus::Paused;
                        }
                    }
                    dialogs::ExtractionDialogResult::Resumed => {
                        if let Some(child) = &self.extraction_child {
                            #[cfg(target_os = "windows")]
                            {
                                let _ = crate::platform::resume_process(child.id());
                            }
                            self.extraction_dialog.status = dialogs::ExtractionStatus::Running;
                        }
                    }
                    dialogs::ExtractionDialogResult::Cancelled => {
                        if let Some(mut child) = self.extraction_child.take() {
                            let _ = child.kill();
                        }
                        self.extraction_rx = None;
                        self.extraction_started = None;
                        self.extraction_dialog.status = dialogs::ExtractionStatus::Cancelled;
                        self.extraction_dialog.show = false;
                        self.status_info.message = "Extraction cancelled".to_string();
                    }
                    dialogs::ExtractionDialogResult::None => {}
                }
            }
        }
        // Password dialog
        if let Some(result) =
            dialogs::render_password_dialog(ctx, &self.theme, &mut self.password_dialog)
        {
            match result {
                dialogs::PasswordDialogResult::Unlock => {
                    if let Some(path) = self.pending_archive_path.clone() {
                        let password = self.password_dialog.password.clone();
                        if archive_operations::try_open_with_password(
                            &self.state,
                            &path,
                            &password,
                            &mut self.password_dialog,
                            &mut self.pending_archive_path,
                            &mut self.status_info,
                            &mut self.entries,
                            &mut self.archive_info,
                        ) {
                            self.password_dialog.show = false;
                            self.pending_archive_path = None;

                            // If we were trying to edit a file, retry now with password
                            if let Some(file_path) = self.pending_edit_file.take() {
                                let archive_opt = {
                                    let st = self.state.lock();
                                    st.current_archive.clone()
                                };
                                if let Some(archive) = archive_opt {
                                    match self.state.lock().read_text_file(&archive, &file_path) {
                                        Ok(text) => {
                                            // Extract just the filename from full path
                                            let name = file_path
                                                .split('/')
                                                .last()
                                                .unwrap_or(&file_path)
                                                .to_string();
                                            self.edit_dialog.show = true;
                                            self.edit_dialog.full_path_in_archive =
                                                file_path.clone();
                                            self.edit_dialog.name_input = name;
                                            self.edit_dialog.content = text;
                                            self.edit_dialog.error.clear();
                                        }
                                        Err(e) => {
                                            self.status_info.message =
                                                format!("Failed to open for edit: {}", e);
                                        }
                                    }
                                }
                            }

                            // If we were trying to open a file, retry now with password
                            if let Some(full_path) = self.pending_open_file.take() {
                                let archive_opt = {
                                    let st = self.state.lock();
                                    st.current_archive.clone()
                                };
                                if let Some(archive) = archive_opt {
                                    // Create FileOpener for smart extraction
                                    let opener = match FileOpener::new() {
                                        Ok(o) => o,
                                        Err(e) => {
                                            self.status_info.message =
                                                format!("Open failed: {}", e);
                                            return;
                                        }
                                    };

                                    // Determine which files to extract (same directory strategy)
                                    let all_entry_paths: Vec<String> = {
                                        let st = self.state.lock();
                                        st.all_entries.iter().map(|e| e.path.clone()).collect()
                                    };

                                    let files_to_extract = opener.get_files_to_extract(
                                        &full_path,
                                        &all_entry_paths,
                                        OpenStrategy::SameDirectory,
                                    );

                                    match self.state.lock().extract_specific(
                                        &archive,
                                        opener.temp_dir(),
                                        files_to_extract,
                                    ) {
                                        Ok(()) => {
                                            let normalized_full_path = full_path.replace(
                                                '/',
                                                std::path::MAIN_SEPARATOR.to_string().as_str(),
                                            );
                                            let file_to_open =
                                                opener.temp_dir().join(&normalized_full_path);

                                            if !file_to_open.exists() {
                                                error!(
                                                    "Extracted file missing: {}",
                                                    file_to_open.display()
                                                );
                                                self.status_info.message =
                                                    "File not found after extraction".to_string();
                                                return;
                                            }

                                            match open::that(&file_to_open) {
                                                Ok(()) => {
                                                    let file_name = full_path
                                                        .split('/')
                                                        .last()
                                                        .unwrap_or(&full_path);
                                                    self.status_info.message =
                                                        format!("Opened {}", file_name);
                                                    std::mem::forget(opener);
                                                }
                                                Err(e) => {
                                                    error!(
                                                        "Failed to launch {}: {}",
                                                        file_to_open.display(),
                                                        e
                                                    );
                                                    self.status_info.message =
                                                        format!("Failed to open file: {}", e);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("Extraction failed: {}", e);
                                            self.status_info.message =
                                                format!("Failed to extract file: {}", e);
                                        }
                                    }
                                }
                            }
                        } else {
                            self.password_dialog.error =
                                "Incorrect password. Please try again.".to_string();
                        }
                    }
                }
                dialogs::PasswordDialogResult::Cancel => {
                    self.password_dialog.show = false;
                    self.pending_archive_path = None;
                    self.pending_edit_file = None;
                    self.pending_open_file = None;
                    self.password_dialog.password.clear();
                    self.password_dialog.error.clear();
                }
            }
        }

        // File Edit dialog
        if let Some(result) =
            dialogs::render_file_edit_dialog(ctx, &self.theme, &mut self.edit_dialog)
        {
            match result {
                dialogs::FileEditResult::Save { new_name, content } => {
                    // Compute destination path in archive (respecting current dir)
                    let dest_full = {
                        let st = self.state.lock();
                        let prefix = st.navigation.current_path.clone();
                        if prefix.is_empty() {
                            new_name.clone()
                        } else {
                            format!("{}/{}", prefix, new_name)
                        }
                    };

                    let archive_opt = {
                        let st = self.state.lock();
                        st.current_archive.clone()
                    };
                    if let Some(archive) = archive_opt {
                        if dest_full != self.edit_dialog.full_path_in_archive {
                            let _ = {
                                self.state.lock().delete_files(
                                    &archive,
                                    &[self.edit_dialog.full_path_in_archive.clone()],
                                )
                            };
                        }
                        let update_res = {
                            self.state
                                .lock()
                                .add_or_update_file_from_str(&archive, &dest_full, &content)
                        };
                        match update_res {
                            Ok(()) => {
                                self.status_info.message = "Saved changes".to_string();
                                self.edit_dialog.show = false;
                                // Reload
                                let mut st = self.state.lock();
                                if let Some(a) = st.current_archive.clone() {
                                    if let Ok(entries) = st.list_archive(&a) {
                                        let current_archive = st.current_archive.clone();
                                        drop(st);
                                        archive_operations::load_archive_data(
                                            &self.state,
                                            entries,
                                            current_archive,
                                            &mut self.password_dialog,
                                            &mut self.pending_archive_path,
                                            &mut self.status_info,
                                            &mut self.entries,
                                            &mut self.archive_info,
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                self.edit_dialog.error = format!("Save failed: {}", e);
                            }
                        }
                    }
                }
                dialogs::FileEditResult::Cancel => {
                    self.edit_dialog.show = false;
                }
            }
        }

        // Password Rules dialog
        if let Some(result) =
            dialogs::render_password_rules_dialog(ctx, &self.theme, &mut self.password_rules_dialog)
        {
            match result {
                dialogs::PasswordRulesResult::Save { rules } => {
                    // Convert UI rules to core PassRule format
                    let pass_rules: Vec<arclain_core::PassRule> = rules
                        .iter()
                        .map(|r| arclain_core::PassRule {
                            name: r.name.clone(),
                            pattern: r.pattern.clone(),
                            password: r.password.clone(),
                            priority: r.priority,
                            enabled: r.enabled,
                        })
                        .collect();

                    // Save to database via state
                    let res = { self.state.lock().save_password_rules(pass_rules) };
                    match res {
                        Ok(()) => {
                            self.password_rules_dialog.show = false;
                            self.password_rules_dialog.error.clear();
                            self.status_info.message = "Password rules saved".to_string();
                        }
                        Err(e) => {
                            self.password_rules_dialog.error = format!("Failed to save: {}", e);
                        }
                    }
                }
                dialogs::PasswordRulesResult::Cancel => {
                    self.password_rules_dialog.show = false;
                    self.password_rules_dialog.error.clear();
                }
            }
        }
    }
}
