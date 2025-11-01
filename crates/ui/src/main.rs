mod features;

use anyhow::Result;
use archust_core::file_opener::{FileOpener, OpenStrategy};
use archust_core::logging::init_logging;
use archust_core::sevenzip::SevenZipCli;
use archust_core::{ArchiveBackend, ConfigStore, NavigationState};
use eframe::egui;
use parking_lot::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, error, info};

use features::{
    dialogs, file_list, header, load_cjk_fonts, properties_panel, status_bar, toolbar, tree_panel,
    AppTheme,
};

struct AppState {
    cfg: ConfigStore,
    backend: SevenZipCli,
    last_entries: Vec<String>,
    all_entries: Vec<archust_core::ArchiveEntry>,
    navigation: NavigationState,
    current_archive: Option<PathBuf>,
    archive_encrypted: bool,
    headers_encrypted: bool,
    encryption_method: Option<String>,
    current_password: Option<String>, // Store the password used to unlock the archive
}

impl AppState {
    fn new() -> Result<Self> {
        info!("Initializing application state");
        let cfg = ConfigStore::load("archust")?;
        debug!("Configuration loaded successfully");

        let backend = SevenZipCli::detect(cfg.cfg.sevenzip_path.as_deref())?;
        info!("7-Zip backend initialized");

        Ok(Self {
            cfg,
            backend,
            last_entries: vec![],
            all_entries: vec![],
            navigation: NavigationState::new(),
            current_archive: None,
            archive_encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
            current_password: None,
        })
    }

    fn list_archive(&mut self, path: &Path) -> Result<Vec<archust_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());

        // Try without password first
        let info = match self.backend.list(path, None) {
            Ok(info) => {
                debug!("Archive opened without password (may have encrypted files inside)");
                info
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let pw = self.cfg.auto_password_for(&self.last_entries);
                if let Some(ref password) = pw {
                    info!("Attempting to open archive with auto-detected password");
                    let info = self.backend.list(path, Some(password))?;
                    // Store this password immediately since we know it worked for listing
                    self.current_password = Some(password.clone());
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };

        // Update entry list
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        
        // Now detect password from the entry list (for archives that list without password but have encrypted files)
        if self.current_password.is_none() {
            let detected_pw = self.cfg.auto_password_for(&self.last_entries);
            if let Some(pwd) = detected_pw {
                self.current_password = Some(pwd);
            }
        }
        
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        

        info!(
            "Archive opened successfully with {} entries",
            self.all_entries.len()
        );
        Ok(self.all_entries.clone())
    }

    fn list_with_password(
        &mut self,
        path: &Path,
        password: &str,
    ) -> Result<Vec<archust_core::ArchiveEntry>> {
        info!("Listing archive with manually provided password");
        let info = self.backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        self.current_password = Some(password.to_string()); // Store the manually entered password
        Ok(self.all_entries.clone())
    }

    fn navigate_to_folder(&mut self, folder: &str) {
        debug!("Navigating to folder: {}", folder);
        self.navigation.navigate_to(folder);
    }

    fn navigate_back(&mut self) {
        debug!("Navigating back from: {}", self.navigation.current_path);
        self.navigation.navigate_back();
    }

    fn navigate_forward(&mut self) {
        debug!("Navigating forward from: {}", self.navigation.current_path);
        self.navigation.navigate_forward();
    }

    fn navigate_up(&mut self) {
        debug!("Navigating up from: {}", self.navigation.current_path);
        self.navigation.navigate_up();
    }

    fn get_current_entries(&self) -> Vec<archust_core::ArchiveEntry> {
        self.navigation.filter_entries(&self.all_entries)
    }

    fn extract_all(&self, archive: &Path, dest: &Path) -> Result<()> {
        info!(
            "Extracting all files from {} to {}",
            archive.display(),
            dest.display()
        );
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend.extract_all(archive, dest, pw)
    }

    fn extract_selected(&self, archive: &Path, dest: &Path, files: Vec<String>) -> Result<()> {
        info!("Extracting {} selected files", files.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());

        let full_paths: Vec<String> = if !self.navigation.current_path.is_empty() {
            files
                .iter()
                .map(|f| format!("{}/{}", self.navigation.current_path, f))
                .collect()
        } else {
            files
        };

        self.backend
            .extract_files(archive, dest, &full_paths, pw)
    }

    // Extract using exact full archive paths provided (no prefixing with navigation path)
    fn extract_specific(&self, archive: &Path, dest: &Path, full_paths: Vec<String>) -> Result<()> {
        info!("Extracting {} file(s) (exact paths)", full_paths.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        // If we have too many files (>100), extract by directory pattern instead
        if full_paths.len() > 100 {
            // Find the common directory
            let dir_path = if let Some(first) = full_paths.first() {
                if let Some(idx) = first.rfind('/') {
                    &first[..idx]
                } else {
                    ""
                }
            } else {
                ""
            };
            info!("Too many files ({}), extracting entire directory: {}", full_paths.len(), if dir_path.is_empty() { "<root>" } else { dir_path });
            self.backend.extract_directory(archive, dest, dir_path, pw)
        } else {
            debug!("Files to extract: {:?}", full_paths);
            self.backend.extract_files(archive, dest, &full_paths, pw)
        }
    }

    fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        self.backend.add_files(archive, &files)
    }

    fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend
            .read_text_file(archive, path_in_archive, pw)
    }

    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        self.backend.delete_files(archive, files)
    }

    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        self.backend
            .add_or_update_file_from_str(archive, path_in_archive, content)
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", size as u64, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

fn convert_to_file_entry(entry: &archust_core::ArchiveEntry) -> file_list::FileEntry {
    let ratio = if entry.size > 0 {
        format!("{}%", (entry.packed_size * 100 / entry.size))
    } else {
        "0%".to_string()
    };

    file_list::FileEntry {
        name: entry.path.clone(),
        size: format_size(entry.size),
        compressed: format_size(entry.packed_size),
        ratio,
        modified: entry.modified.clone().unwrap_or_default(),
        encrypted: entry.encrypted,
        is_folder: entry.is_dir,
        selected: false,
    }
}

struct ArchustApp {
    state: Arc<Mutex<AppState>>,
    theme: AppTheme,

    // UI State
    header_state: header::HeaderState,
    toolbar_state: toolbar::ToolbarState,
    tree_state: tree_panel::TreePanelState,
    password_dialog: dialogs::PasswordDialog,
    edit_dialog: dialogs::FileEditDialog,

    // Data
    entries: Vec<file_list::FileEntry>,
    status_info: status_bar::StatusBarInfo,
    archive_loaded: bool,
    current_path: String,
    pending_archive_path: Option<PathBuf>,
    pending_edit_file: Option<String>, // Track file that needs editing after password unlock
    pending_open_file: Option<String>, // Track file that needs opening after password unlock

    // Archive info
    archive_format: String,
    total_size: u64,
    compressed_size: u64,
    file_count: usize,
    archive_encrypted: bool,
    headers_encrypted: bool,
    encryption_method: Option<String>,
}

impl ArchustApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
            header_state: header::HeaderState::default(),
            toolbar_state: toolbar::ToolbarState::default(),
            tree_state: tree_panel::TreePanelState::default(),
            password_dialog: dialogs::PasswordDialog::default(),
            edit_dialog: dialogs::FileEditDialog::default(),
            entries: Vec::new(),
            status_info: status_bar::StatusBarInfo::default(),
            archive_loaded: false,
            current_path: String::new(),
            pending_archive_path: None,
            pending_edit_file: None,
            pending_open_file: None,
            archive_format: String::new(),
            total_size: 0,
            compressed_size: 0,
            file_count: 0,
            archive_encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        }
    }

    fn open_archive(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("Archives", &["zip", "7z", "rar"])
            .pick_file()
        {
            info!("File selected: {}", file.display());
            self.current_path = file.to_string_lossy().to_string();

            let mut state = self.state.lock();
            match state.list_archive(&file) {
                Ok(archive_entries) => {
                    let current_archive = state.current_archive.clone();
                    drop(state);
                    self.load_archive_data(archive_entries, current_archive);
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    if err_msg.contains("Wrong password")
                        || err_msg.contains("Cannot open encrypted archive")
                    {
                        self.password_dialog.show = true;
                        self.pending_archive_path = Some(file);
                        self.password_dialog.password.clear();
                        self.password_dialog.error.clear();
                        self.status_info.message = "Archive is password-protected".to_string();
                    } else {
                        error!("Failed to load archive: {}", err_msg);
                        self.status_info.message = format!("Failed to load archive: {}", err_msg);
                    }
                }
            }
        }
    }

    fn try_open_with_password(&mut self, path: &PathBuf, password: &str) -> bool {
        let mut state = self.state.lock();
        // Save the current navigation state before re-listing
        let saved_current_path = state.navigation.current_path.clone();
        let saved_path_stack = state.navigation.path_stack.clone();
        
        match state.list_with_password(path, password) {
            Ok(archive_entries) => {
                // Restore navigation state after re-listing
                state.navigation.current_path = saved_current_path;
                state.navigation.path_stack = saved_path_stack;
                state.navigation.forward_stack.clear(); // Clear forward stack as we're not navigating
                
                let current_archive = state.current_archive.clone();
                drop(state);
                self.load_archive_data(archive_entries, current_archive);
                true
            }
            Err(_) => false,
        }
    }

    fn load_archive_data(
        &mut self,
        archive_entries: Vec<archust_core::ArchiveEntry>,
        current_archive: Option<PathBuf>,
    ) {
        let state = self.state.lock();
        self.entries = state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();
        self.archive_encrypted = state.archive_encrypted;
        self.headers_encrypted = state.headers_encrypted;
        self.encryption_method = state.encryption_method.clone();
        drop(state);

        self.total_size = archive_entries.iter().map(|e| e.size).sum();
        self.compressed_size = archive_entries.iter().map(|e| e.packed_size).sum();
        self.file_count = archive_entries.len();

        if let Some(archive_path) = &current_archive {
            self.archive_format = archive_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| "Archive".to_string());
        }

        self.archive_loaded = true;
        self.status_info.message = "Archive loaded successfully".to_string();
        self.status_info.file_count = self.file_count;
        self.status_info.total_size = format_size(self.total_size);
        self.status_info.compressed_size = format_size(self.compressed_size);
        self.status_info.archive_format = self.archive_format.clone();
    }

    fn navigate_to(&mut self, folder: &str) {
        let mut state = self.state.lock();
        state.navigate_to_folder(folder);
        self.entries = state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();

        let current_path = state.navigation.current_path.clone();
        let current_archive = state.current_archive.clone();
        drop(state);

        self.update_current_path(current_path, current_archive);
    }

    fn navigate_back(&mut self) {
        let mut state = self.state.lock();
        state.navigate_back();
        self.entries = state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();

        let current_path = state.navigation.current_path.clone();
        let current_archive = state.current_archive.clone();
        drop(state);

        self.update_current_path(current_path, current_archive);
    }

    fn navigate_forward(&mut self) {
        let mut state = self.state.lock();
        state.navigate_forward();
        self.entries = state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();

        let current_path = state.navigation.current_path.clone();
        let current_archive = state.current_archive.clone();
        drop(state);

        self.update_current_path(current_path, current_archive);
    }

    fn navigate_up(&mut self) {
        let mut state = self.state.lock();
        state.navigate_up();
        self.entries = state
            .get_current_entries()
            .iter()
            .map(convert_to_file_entry)
            .collect();

        let current_path = state.navigation.current_path.clone();
        let current_archive = state.current_archive.clone();
        drop(state);

        self.update_current_path(current_path, current_archive);
    }

    fn update_current_path(&mut self, current_path: String, current_archive: Option<PathBuf>) {
        self.current_path = if current_path.is_empty() {
            current_archive
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            format!(
                "{} > {}",
                current_archive
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default(),
                current_path
            )
        };
    }

    fn extract_selected(&mut self) {
        let state = self.state.lock();
        if let Some(archive) = &state.current_archive {
            let selected_files: Vec<String> = self
                .entries
                .iter()
                .filter(|e| e.selected)
                .map(|e| e.name.clone())
                .collect();

            if selected_files.is_empty() {
                self.status_info.message = "No files selected".to_string();
                return;
            }

            let archive_clone = archive.clone();
            drop(state);

            if let Some(dest) = rfd::FileDialog::new().pick_folder() {
                self.status_info.message = format!("Extracting {} files...", selected_files.len());
                let state = self.state.lock();
                match state.extract_selected(&archive_clone, &dest, selected_files) {
                    Ok(()) => {
                        self.status_info.message = format!("Extracted to {}", dest.display());
                    }
                    Err(e) => {
                        self.status_info.message = format!("Extract failed: {}", e);
                    }
                }
            }
        }
    }

    fn add_files(&mut self) {
        let archive_path = {
            let state = self.state.lock();
            state.current_archive.clone()
        };

        if let Some(archive) = archive_path {
            if let Some(files) = rfd::FileDialog::new().pick_files() {
                let state = self.state.lock();
                match state.add_files_to_archive(&archive, files) {
                    Ok(()) => {
                        self.status_info.message = "Files added successfully".to_string();
                    }
                    Err(e) => {
                        self.status_info.message = format!("Add files failed: {}", e);
                    }
                }
            }
        }
    }
}

impl eframe::App for ArchustApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme
        self.theme.apply_to_context(ctx);

        // Header
        egui::TopBottomPanel::top("header")
            .exact_height(52.0)
            .frame(
                egui::Frame::none()
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)),
            )
            .show(ctx, |ui| {
                let mut toggle_theme = false;
                header::render(ui, &self.theme, &mut self.header_state, &mut toggle_theme);

                if toggle_theme {
                    self.theme.toggle();
                }
            });

        // Toolbar
        egui::TopBottomPanel::top("toolbar")
            .exact_height(52.0)
            .frame(
                egui::Frame::none()
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                    .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)),
            )
            .show(ctx, |ui| {
                let state = self.state.lock();
                let can_go_back = state.navigation.can_go_back();
                let can_go_forward = state.navigation.can_go_forward();
                let can_go_up = state.navigation.can_go_up();
                drop(state);

                let actions = toolbar::render(
                    ui,
                    &self.theme,
                    &mut self.toolbar_state,
                    can_go_back,
                    can_go_forward,
                    can_go_up,
                    self.archive_loaded,
                );

                if actions.open {
                    self.open_archive();
                }
                if actions.go_back {
                    self.navigate_back();
                }
                if actions.go_forward {
                    self.navigate_forward();
                }
                if actions.go_up {
                    self.navigate_up();
                }
                if actions.extract {
                    self.extract_selected();
                }
                if actions.add {
                    self.add_files();
                }
            });

        // Status bar
        egui::TopBottomPanel::bottom("status")
            .exact_height(32.0)
            .frame(
                egui::Frame::none()
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(0.0, 8.0)),
            )
            .show(ctx, |ui| {
                status_bar::render(ui, &self.theme, &self.status_info, self.archive_loaded);
            });

        // Left panel - Tree view
        if self.toolbar_state.show_tree_panel && self.archive_loaded {
            egui::SidePanel::left("tree_panel")
                .exact_width(240.0)
                .frame(egui::Frame::none().fill(self.theme.colors.bg_secondary))
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
                            self.update_current_path(String::new(), current_archive);
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
                            self.update_current_path(path, current_archive);
                        }
                    }
                });
        }

        // Right panel - Properties
        if self.toolbar_state.show_properties_panel && self.archive_loaded {
            egui::SidePanel::right("properties_panel")
                .exact_width(280.0)
                .frame(
                    egui::Frame::none()
                        .fill(self.theme.colors.bg_secondary)
                        .inner_margin(egui::Margin::symmetric(16.0, 16.0)),
                )
                .show(ctx, |ui| {
                    let groups = vec![properties_panel::create_archive_info_group(
                        &self.archive_format,
                        self.file_count,
                        &format_size(self.total_size),
                        &format_size(self.compressed_size),
                        self.archive_encrypted,
                        self.headers_encrypted,
                        self.encryption_method.as_deref(),
                    )];

                    properties_panel::render(ui, &self.theme, &groups);
                });
        }

        // Central panel - File list
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.theme.colors.bg_primary))
            .show(ctx, |ui| {
                if !self.archive_loaded {
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
                        egui::Frame::none()
                            .fill(self.theme.colors.bg_secondary)
                            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
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
                                            file_list::FileListAction::Navigate(folder) => self.navigate_to(&folder),
                                            file_list::FileListAction::Open(name) => {
                                                // Reuse the list-view handler by constructing the same flow via the same block below.
                                                // To avoid duplication, we call the same code path by pushing this action to be handled in place.
                                                // Inline the same open code here for clarity.
                                                let full_path = {
                                                    let st = self.state.lock();
                                                    let prefix = st.navigation.current_path.clone();
                                                    if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) }
                                                };
                                                // Pre-check encryption and prompt before attempting extraction
                                                let need_pw = {
                                                    let st = self.state.lock();
                                                    let is_encrypted = st.all_entries.iter().any(|e| e.path == full_path && e.encrypted);
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(&st.last_entries).is_some();
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
                                    ) {
                                        match action {
                                            file_list::FileListAction::Navigate(folder) => {
                                                self.navigate_to(&folder)
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
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(&st.last_entries).is_some();
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
                                                    let have_pw = st.current_password.is_some() || st.cfg.auto_password_for(&st.last_entries).is_some();
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
                                                
                                                let files_to_extract = opener.get_files_to_extract(
                                                    &full_path,
                                                    &all_entry_paths,
                                                    OpenStrategy::SameDirectory,
                                                );
                                                
                                                info!("Opening file: {} (extracting {} related files)", name, files_to_extract.len());

                                                // Extract files
                                                let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                                                if let Some(archive) = archive_opt {
                                                    let res = { self.state.lock().extract_specific(&archive, opener.temp_dir(), files_to_extract) };
                                                    match res {
                                                        Ok(()) => {
                                                            info!("Extraction completed successfully, preparing to open {}", full_path);
                                                            let normalized_full_path = full_path.replace('/', std::path::MAIN_SEPARATOR.to_string().as_str());
                                                            let file_to_open = opener.temp_dir().join(&normalized_full_path);
                                                            
                                                            if !file_to_open.exists() {
                                                                error!("Extracted file missing: {}", file_to_open.display());
                                                                self.status_info.message = "File not found after extraction".to_string();
                                                                return;
                                                            }
                                                            
                                                            info!("Launching extracted file: {}", file_to_open.display());
                                                            match open::that(&file_to_open) {
                                                                Ok(()) => {
                                                                    info!("Successfully opened {}", file_to_open.display());
                                                                    self.status_info.message = format!("Opened {}", name);
                                                                    std::mem::forget(opener);
                                                                }
                                                                Err(e) => {
                                                                    error!("Failed to launch {}: {}", file_to_open.display(), e);
                                                                    self.status_info.message = format!("Failed to open file: {}", e);
                                                                }
                                                            }
                                                        }
                                                        Err(e) => {
                                                            let err_msg = e.to_string();
                                                            error!("Extraction failed: {}", err_msg);
                                                            // Check if it's a password error - 7z returns exit code 2 for password errors
                                                            if err_msg.contains("Wrong password")
                                                                || err_msg.contains("Cannot open encrypted")
                                                                || err_msg.contains("code Some(2)") {
                                                                // Prompt for password, then retry open
                                                                info!("Showing password dialog for encrypted file (detected password error)");
                                                                self.password_dialog.show = true;
                                                                self.password_dialog.password.clear();
                                                                self.password_dialog.error.clear();
                                                                self.pending_archive_path = { let st = self.state.lock(); st.current_archive.clone() };
                                                                // Remember target to open after unlock
                                                                self.pending_open_file = Some(full_path.clone());
                                                                info!("Stored pending_open_file: {}", full_path);
                                                                self.status_info.message = "Password required to open file".to_string();
                                                            } else {
                                                                self.status_info.message = format!("Open failed: {}", err_msg);
                                                            }
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
                                                                self.load_archive_data(entries, current_archive);
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
                        self.update_current_path(path, current_archive);
                    }
                }
            });

        // Password dialog
        if let Some(result) =
            dialogs::render_password_dialog(ctx, &self.theme, &mut self.password_dialog)
        {
            match result {
            dialogs::PasswordDialogResult::Unlock => {
                info!("Password dialog unlock clicked");
                if let Some(path) = self.pending_archive_path.clone() {
                    let password = self.password_dialog.password.clone();
                    info!("Attempting to unlock archive with provided password (length: {})", password.len());
                    if self.try_open_with_password(&path, &password) {
                        info!("Archive unlocked successfully");
                        self.password_dialog.show = false;
                        self.pending_archive_path = None;
                        
                        // If we were trying to edit a file, retry now with password
                        if let Some(file_path) = self.pending_edit_file.take() {
                            info!("Retrying edit for file: {}", file_path);
                            let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                            if let Some(archive) = archive_opt {
                                match self.state.lock().read_text_file(&archive, &file_path) {
                                    Ok(text) => {
                                        // Extract just the filename from full path
                                        let name = file_path.split('/').last().unwrap_or(&file_path).to_string();
                                        self.edit_dialog.show = true;
                                        self.edit_dialog.full_path_in_archive = file_path.clone();
                                        self.edit_dialog.name_input = name;
                                        self.edit_dialog.content = text;
                                        self.edit_dialog.error.clear();
                                    }
                                    Err(e) => {
                                        self.status_info.message = format!("Failed to open for edit: {}", e);
                                    }
                                }
                            }
                        }
                        
                        // If we were trying to open a file, retry now with password
                        if let Some(full_path) = self.pending_open_file.take() {
                            info!("Found pending_open_file, retrying: {}", full_path);
                            let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                            if let Some(archive) = archive_opt {
                                // Create FileOpener for smart extraction
                                let opener = match FileOpener::new() {
                                    Ok(o) => o,
                                    Err(e) => {
                                        self.status_info.message = format!("Open failed: {}", e);
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
                                
                                info!("Retrying open: {} (extracting {} related files)", full_path, files_to_extract.len());
                                
                                match self.state.lock().extract_specific(&archive, opener.temp_dir(), files_to_extract) {
                                    Ok(()) => {
                                        info!("Extraction completed successfully, opening file {}", full_path);
                                        let normalized_full_path = full_path.replace('/', std::path::MAIN_SEPARATOR.to_string().as_str());
                                        let file_to_open = opener.temp_dir().join(&normalized_full_path);
                                        
                                        info!("Looking for extracted file at: {}", file_to_open.display());
                                        
                                        if !file_to_open.exists() {
                                            error!("Extracted file missing: {}", file_to_open.display());
                                            self.status_info.message = "File not found after extraction".to_string();
                                            return;
                                        }
                                        
                                        match open::that(&file_to_open) {
                                            Ok(()) => {
                                                let file_name = full_path.split('/').last().unwrap_or(&full_path);
                                                info!("Successfully opened {}", file_name);
                                                self.status_info.message = format!("Opened {}", file_name);
                                                std::mem::forget(opener);
                                            }
                                            Err(e) => {
                                                error!("Failed to launch {}: {}", file_to_open.display(), e);
                                                self.status_info.message = format!("Failed to open file: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Extraction failed: {}", e);
                                        self.status_info.message = format!("Failed to extract file: {}", e);
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
        if let Some(result) = dialogs::render_file_edit_dialog(ctx, &self.theme, &mut self.edit_dialog) {
            match result {
                dialogs::FileEditResult::Save { new_name, content } => {
                    // Compute destination path in archive (respecting current dir)
                    let dest_full = {
                        let st = self.state.lock();
                        let prefix = st.navigation.current_path.clone();
                        if prefix.is_empty() { new_name.clone() } else { format!("{}/{}", prefix, new_name) }
                    };

                    let archive_opt = { let st = self.state.lock(); st.current_archive.clone() };
                    if let Some(archive) = archive_opt {
                        if dest_full != self.edit_dialog.full_path_in_archive {
                            let _ = { self.state.lock().delete_files(&archive, &[self.edit_dialog.full_path_in_archive.clone()]) };
                        }
                        let update_res = { self.state.lock().add_or_update_file_from_str(&archive, &dest_full, &content) };
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
                                        self.load_archive_data(entries, current_archive);
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
    }
}

#[cfg(target_os = "windows")]
fn detect_dark_mode() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) =
        hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
    {
        if let Ok(value) = key.get_value::<u32, _>("AppsUseLightTheme") {
            return value == 0;
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
fn detect_dark_mode() -> bool {
    false
}

fn main() -> Result<()> {
    if let Err(e) = init_logging() {
        eprintln!("Failed to initialize logging: {}", e);
    }

    info!("Starting Archust application");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Archust - Archive Viewer")
            .with_visible(true)
            .with_resizable(true)
            .with_decorations(true),
        ..Default::default()
    };

    eframe::run_native(
        "Archust",
        options,
        Box::new(|cc| Ok(Box::new(ArchustApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run app: {}", e))?;

    info!("Application shutting down");
    Ok(())
}
