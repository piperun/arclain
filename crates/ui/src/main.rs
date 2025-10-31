mod features;

use archust_core::{ConfigStore, ArchiveBackend, ArchiveKind, NavigationState};
use archust_core::sevenzip::SevenZipCli;
use archust_core::logging::init_logging;
use parking_lot::Mutex;
use std::{sync::Arc, path::{PathBuf, Path}};
use anyhow::Result;
use regex::Regex;
use tracing::{info, warn, error, debug};
use eframe::egui;

use features::{
    AppTheme,
    header, toolbar, tree_panel, file_list, properties_panel, status_bar, dialogs,
};

struct AppState {
    cfg: ConfigStore,
    backend: SevenZipCli,
    last_entries: Vec<String>,
    all_entries: Vec<archust_core::ArchiveEntry>,
    navigation: NavigationState,
    current_archive: Option<PathBuf>,
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
        })
    }

    fn list_archive(&mut self, path: &Path) -> Result<Vec<archust_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        
        let info = self.backend.list(path, None).or_else(|e| {
            debug!("Initial listing failed, trying with auto-password: {}", e);
            let pw = self.cfg.auto_password_for(&self.last_entries);
            if pw.is_some() {
                info!("Attempting to open archive with auto-detected password");
            }
            self.backend.list(path, pw.as_deref())
        })?;

        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.navigation = NavigationState::new();

        info!("Archive opened successfully with {} entries", self.all_entries.len());
        Ok(self.all_entries.clone())
    }

    fn list_with_password(&mut self, path: &Path, password: &str) -> Result<Vec<archust_core::ArchiveEntry>> {
        let info = self.backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.navigation = NavigationState::new();
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
        info!("Extracting all files from {} to {}", archive.display(), dest.display());
        let pw = self.cfg.auto_password_for(&self.last_entries);
        self.backend.extract_all(archive, dest, pw.as_deref())
    }

    fn extract_selected(&self, archive: &Path, dest: &Path, files: Vec<String>) -> Result<()> {
        info!("Extracting {} selected files", files.len());
        let pw = self.cfg.auto_password_for(&self.last_entries);
        
        let full_paths: Vec<String> = if !self.navigation.current_path.is_empty() {
            files.iter().map(|f| format!("{}/{}", self.navigation.current_path, f)).collect()
        } else {
            files
        };
        
        self.backend.extract_files(archive, dest, &full_paths, pw.as_deref())
    }

    fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        self.backend.add_files(archive, &files)
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
    
    // Data
    entries: Vec<file_list::FileEntry>,
    status_info: status_bar::StatusBarInfo,
    archive_loaded: bool,
    current_path: String,
    pending_archive_path: Option<PathBuf>,
    
    // Archive info
    archive_format: String,
    total_size: u64,
    compressed_size: u64,
    file_count: usize,
}

impl ArchustApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);
        theme.apply_to_context(&cc.egui_ctx);
        
        let state = Arc::new(Mutex::new(AppState::new().expect("Failed to initialize app state")));

        Self {
            state,
            theme,
            header_state: header::HeaderState::default(),
            toolbar_state: toolbar::ToolbarState::default(),
            tree_state: tree_panel::TreePanelState::default(),
            password_dialog: dialogs::PasswordDialog::default(),
            entries: Vec::new(),
            status_info: status_bar::StatusBarInfo::default(),
            archive_loaded: false,
            current_path: String::new(),
            pending_archive_path: None,
            archive_format: String::new(),
            total_size: 0,
            compressed_size: 0,
            file_count: 0,
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
                    if err_msg.contains("Wrong password") || err_msg.contains("Cannot open encrypted archive") {
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
        match state.list_with_password(path, password) {
            Ok(archive_entries) => {
                let current_archive = state.current_archive.clone();
                drop(state);
                self.load_archive_data(archive_entries, current_archive);
                true
            }
            Err(_) => {
                false
            }
        }
    }

    fn load_archive_data(&mut self, archive_entries: Vec<archust_core::ArchiveEntry>, current_archive: Option<PathBuf>) {
        let state = self.state.lock();
        self.entries = state.get_current_entries().iter()
            .map(convert_to_file_entry)
            .collect();
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
        self.entries = state.get_current_entries().iter()
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
        self.entries = state.get_current_entries().iter()
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
        self.entries = state.get_current_entries().iter()
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
        self.entries = state.get_current_entries().iter()
            .map(convert_to_file_entry)
            .collect();
        
        let current_path = state.navigation.current_path.clone();
        let current_archive = state.current_archive.clone();
        drop(state);
        
        self.update_current_path(current_path, current_archive);
    }

    fn update_current_path(&mut self, current_path: String, current_archive: Option<PathBuf>) {
        self.current_path = if current_path.is_empty() {
            current_archive.as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            format!("{} > {}",
                current_archive.as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default(),
                current_path)
        };
    }

    fn extract_selected(&mut self) {
        let state = self.state.lock();
        if let Some(archive) = &state.current_archive {
            let selected_files: Vec<String> = self.entries.iter()
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
            .frame(egui::Frame::none()
                .fill(self.theme.colors.bg_secondary)
                .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)))
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
            .frame(egui::Frame::none()
                .fill(self.theme.colors.bg_secondary)
                .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                .stroke(egui::Stroke::new(1.0, self.theme.colors.border_color)))
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
            .frame(egui::Frame::none()
                .fill(self.theme.colors.bg_secondary)
                .inner_margin(egui::Margin::symmetric(0.0, 8.0)))
            .show(ctx, |ui| {
                status_bar::render(ui, &self.theme, &self.status_info, self.archive_loaded);
            });

        // Left panel - Tree view
        if self.toolbar_state.show_tree_panel && self.archive_loaded {
            egui::SidePanel::left("tree_panel")
                .exact_width(240.0)
                .frame(egui::Frame::none()
                    .fill(self.theme.colors.bg_secondary))
                .show(ctx, |ui| {
                    let state = self.state.lock();
                    let archive_name = state.current_archive.as_ref()
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
                            self.entries = state.get_current_entries().iter()
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
                            self.entries = state.get_current_entries().iter()
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
                .frame(egui::Frame::none()
                    .fill(self.theme.colors.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(16.0, 16.0)))
                .show(ctx, |ui| {
                    let groups = vec![
                        properties_panel::create_archive_info_group(
                            &self.archive_format,
                            self.file_count,
                            &format_size(self.total_size),
                            &format_size(self.compressed_size),
                        ),
                    ];
                    
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
                            ui.label(egui::RichText::new("No archive loaded")
                                .size(18.0)
                                .color(self.theme.colors.text_primary));
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Click 'Open' to load an archive")
                                .size(14.0)
                                .color(self.theme.colors.text_secondary));
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
                                let archive_name = state.current_archive.as_ref()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let current_path = state.navigation.current_path.clone();
                                drop(state);
                                
                                breadcrumb_nav = file_list::render_breadcrumb(ui, &self.theme, &current_path, &archive_name);
                            });
                        
                        // File list scroll area
                        egui::ScrollArea::vertical()
                            .id_salt("file_list_scroll")
                            .show(ui, |ui| {
                                let navigate_to = if self.toolbar_state.grid_view {
                                    file_list::render_grid_view(ui, &self.theme, &mut self.entries)
                                } else {
                                    file_list::render_list_view(ui, &self.theme, &mut self.entries)
                                };
                                
                                if let Some(folder) = navigate_to {
                                    self.navigate_to(&folder);
                                }
                            });
                    });
                    
                    // Handle breadcrumb navigation
                    if let Some(path) = breadcrumb_nav {
                        // Direct navigation to the clicked path
                        let mut state = self.state.lock();
                        state.navigation.set_current_path(&path);
                        state.navigation.forward_stack.clear();
                        self.entries = state.get_current_entries().iter()
                            .map(convert_to_file_entry)
                            .collect();
                        let current_archive = state.current_archive.clone();
                        drop(state);
                        self.update_current_path(path, current_archive);
                    }
                }
            });

        // Password dialog
        if let Some(result) = dialogs::render_password_dialog(ctx, &self.theme, &mut self.password_dialog) {
            match result {
                dialogs::PasswordDialogResult::Unlock => {
                    if let Some(path) = self.pending_archive_path.clone() {
                        let password = self.password_dialog.password.clone();
                        if self.try_open_with_password(&path, &password) {
                            self.password_dialog.show = false;
                            self.pending_archive_path = None;
                        } else {
                            self.password_dialog.error = "Incorrect password. Please try again.".to_string();
                        }
                    }
                }
                dialogs::PasswordDialogResult::Cancel => {
                    self.password_dialog.show = false;
                    self.pending_archive_path = None;
                    self.password_dialog.password.clear();
                    self.password_dialog.error.clear();
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn detect_dark_mode() -> bool {
    use winreg::RegKey;
    use winreg::enums::*;
    
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize") {
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
    
    info!("Starting Archust application with feature-first architecture");
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Archust - Archive Viewer"),
        ..Default::default()
    };
    
    eframe::run_native(
        "Archust",
        options,
        Box::new(|cc| Ok(Box::new(ArchustApp::new(cc)))),
    ).map_err(|e| anyhow::anyhow!("Failed to run app: {}", e))?;
    
    info!("Application shutting down");
    Ok(())
}