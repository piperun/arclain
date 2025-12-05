//! Main application coordinator
//!
//! The ArclainApp struct serves as the primary coordination point for the entire UI,
//! managing global state and delegating rendering to feature modules.

use crate::core::{
    navigation::{AppPage, PageNavigator, SettingsPage},
    operations,
    state::AppState,
};
use crate::features::{organization, password_management, plugins, settings};
use crate::platform::detect_dark_mode;
use crate::shared::{components, dialogs, theme::AppTheme};

use eframe::egui;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// Main application struct coordinating all features
pub struct ArclainApp {
    // Core state
    pub(crate) state: Arc<Mutex<AppState>>,
    pub(crate) theme: AppTheme,

    // Navigation
    pub(crate) page_navigator: PageNavigator,

    // Feature states will be added here as we extract them
    // TODO: Add feature-specific state structs

    // Temporary: Keep all state here until extraction is complete
    // UI State
    header_state: components::HeaderState,
    // Feature states
    archive_browser: crate::features::archive_browser::ArchiveBrowser,
    archive_operations: crate::features::archive_operations::ArchiveOperations,

    // Dialog states
    password_feature: password_management::PasswordFeature,
    edit_dialog: crate::features::file_editing::FileEditDialog,
    // password_rules_dialog: password_management::PasswordRulesDialog, // Moved to SettingsFeature

    // Settings state
    settings_feature: settings::SettingsFeature,
    // security_settings_state: settings::SecuritySettingsState,
    // archives_settings_state: settings::ArchivesSettingsState,
    // plugins_state: plugins::PluginsListState, // Moved to SettingsFeature
    plugins_feature: plugins::PluginsFeature,
    organization_feature: organization::OrganizationFeature,

    // Data
    status_info: components::StatusBarInfo,
    _pending_archive_path: Option<PathBuf>,
    _pending_edit_file: Option<String>,
    // _pending_open_file: Option<String>, // Moved to ArchiveOperations

    // Archive info
    // pub(crate) archive_info: operations::archive::ArchiveInfo, // Duplicated from AppState
    _last_window_title: Option<String>,

    // Extraction progress state - Moved to ArchiveOperations
    // extraction_dialog: dialogs::progress::ExtractionProgressDialog,
    // _extraction_rx: Option<Receiver<ProgressUpdate>>,
    // _extraction_child: Option<std::process::Child>,
    // extraction_minimized: bool,
    // _extraction_started: Option<Instant>,
    _password_rules_loaded: bool,
}

impl ArclainApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);

        // Load CJK fonts during initialization to support Japanese/Chinese characters
        crate::shared::theme::load_cjk_fonts(&cc.egui_ctx);

        let state = Arc::new(Mutex::new(
            AppState::new().expect("Failed to initialize app state"),
        ));

        let shared_state = crate::shared::SharedState {
            app_state: state.clone(),
            theme: theme.clone(),
        };

        Self {
            state,
            theme,
            page_navigator: PageNavigator::new(),
            header_state: components::HeaderState::default(),
            archive_browser: crate::features::archive_browser::ArchiveBrowser::new(&shared_state),
            archive_operations: crate::features::archive_operations::ArchiveOperations::new(&shared_state),
            password_feature: password_management::PasswordFeature::new(&shared_state),
            edit_dialog: crate::features::file_editing::FileEditDialog::default(),
            // password_rules_dialog: password_management::PasswordRulesDialog::default(),
            settings_feature: settings::SettingsFeature::new(&shared_state),
            // security_settings_state: settings::SecuritySettingsState::default(),
            // archives_settings_state: settings::ArchivesSettingsState::default(),
            // plugins_state: plugins::PluginsListState::default(),
            plugins_feature: plugins::PluginsFeature::new(&shared_state),
            organization_feature: organization::OrganizationFeature::new(&shared_state),
            status_info: components::StatusBarInfo::default(),
            _pending_archive_path: None,
            _pending_edit_file: None,
            // _pending_open_file: None,
            // archive_info: operations::archive::ArchiveInfo::default(),
            _last_window_title: None,
            // extraction_dialog: dialogs::progress::ExtractionProgressDialog::default(),
            // _extraction_rx: None,
            // _extraction_child: None,
            // extraction_minimized: false,
            // _extraction_started: None,
            _password_rules_loaded: false,
        }
    }
}

// TODO: Implement eframe::App trait
// For now, this is just a stub that will be filled in as we extract feature rendering
impl eframe::App for ArclainApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme
        self.theme.apply_to_context(ctx);

        // Update window title
        let title = {
            let state = self.state.lock();
            if let Some(path) = &state.current_archive {
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Arclain".to_string())
            } else if let Some(settings_page) = self.page_navigator.current_settings_page() {
                format!("Settings - {}", settings_page.display_name())
            } else {
                "Arclain".to_string()
            }
        };
        
        let sanitized_title = crate::core::operations::window::sanitize_window_title(&title);
        if self._last_window_title.as_deref() != Some(&sanitized_title) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(sanitized_title.clone()));
            self._last_window_title = Some(sanitized_title);
        }

        // Handle extraction progress
        self.archive_operations.update_extraction_progress(ctx);
        self.archive_operations.update_conversion_progress(ctx);

        // Render Header
        egui::TopBottomPanel::top("header_panel")
            .frame(egui::Frame::NONE.fill(self.theme.colors.bg_secondary))
            .show(ctx, |ui| {
                let mut theme_toggle = false;
                let can_go_back = self.page_navigator.can_go_back();
                let is_on_settings = self.page_navigator.is_on_settings();
                
                let actions = components::header::render(
                    ui,
                    &self.theme,
                    &mut self.header_state,
                    &mut theme_toggle,
                    true, // Always show nav buttons for now
                    can_go_back,
                    is_on_settings,
                );

                if theme_toggle {
                    self.theme.toggle();
                }

                if actions.navigate_home {
                    self.page_navigator.navigate_to_main();
                }
                if actions.navigate_back {
                    self.page_navigator.navigate_back();
                }
                if actions.navigate_plugins {
                    self.page_navigator.navigate_to(AppPage::Plugins);
                }
                if actions.navigate_settings {
                    self.page_navigator.navigate_to(AppPage::Settings(SettingsPage::Overview));
                }
            });

        // Render Toolbar (only on Main page)
        if self.page_navigator.is_on_main() {
            egui::TopBottomPanel::top("toolbar_panel")
                .frame(egui::Frame::NONE.fill(self.theme.colors.bg_secondary))
                .show(ctx, |ui| {
                    let state = self.state.lock();
                    let can_go_back = state.navigation.can_go_back();
                    let can_go_forward = state.navigation.can_go_forward();
                    let can_go_up = state.navigation.can_go_up();
                    let archive_loaded = state.current_archive.is_some();
                    let has_selection = false; // TODO: Implement selection tracking
                    let has_metadata = state.plugin_metadata.is_some();
                    drop(state);

                    let actions = components::toolbar::render(
                        ui,
                        &self.theme,
                        &mut self.archive_browser.state_mut().toolbar_state,
                        can_go_back,
                        can_go_forward,
                        can_go_up,
                        archive_loaded,
                        has_selection,
                        has_metadata,
                    );

                    // Handle toolbar actions
                    let shared_state = crate::shared::SharedState {
                        app_state: self.state.clone(),
                        theme: self.theme.clone(),
                    };

                    if actions.go_back {
                        crate::features::archive_browser::navigation::navigate_back(
                            self.archive_browser.state_mut(),
                            &shared_state,
                        );
                    }
                    if actions.go_forward {
                        crate::features::archive_browser::navigation::navigate_forward(
                            self.archive_browser.state_mut(),
                            &shared_state,
                        );
                    }
                    if actions.go_up {
                        crate::features::archive_browser::navigation::navigate_up(
                            self.archive_browser.state_mut(),
                            &shared_state,
                        );
                    }
                    if actions.open {
                        let mut archive_info = operations::archive::ArchiveInfo::default();
                        let browser_state = self.archive_browser.state_mut();
                        operations::archive::open_archive(
                            &self.state,
                            &mut browser_state.current_path,
                            &mut self.password_feature.password_dialog,
                            &mut self._pending_archive_path,
                            &mut self.status_info,
                            &mut browser_state.entries,
                            &mut archive_info,
                        );
                    }
                    if actions.extract {
                        let browser_state = self.archive_browser.state_mut();
                        let ops_state = self.archive_operations.state_mut();
                        operations::extraction::extract_selected(
                            &self.state,
                            &browser_state.entries,
                            &mut ops_state.extraction_dialog,
                            &mut ops_state.extraction_rx,
                            &mut ops_state.extraction_child,
                            &mut ops_state.extraction_minimized,
                            &mut ops_state.extraction_started,
                            &mut self.status_info,
                        );
                    }
                    if actions.extract_all {
                        let ops_state = self.archive_operations.state_mut();
                        operations::extraction::extract_all(
                            &self.state,
                            &mut ops_state.extraction_dialog,
                            &mut ops_state.extraction_rx,
                            &mut ops_state.extraction_child,
                            &mut ops_state.extraction_minimized,
                            &mut ops_state.extraction_started,
                            &mut self.status_info,
                        );
                    }
                    if actions.add {
                        operations::file::add_files(&self.state, &mut self.status_info);
                    }
                    if actions.delete_selected {
                        let mut archive_info = operations::archive::ArchiveInfo::default();
                        let browser_state = self.archive_browser.state_mut();
                        let entries_clone = browser_state.entries.clone();
                        operations::file::delete_selected(
                            &self.state,
                            &entries_clone,
                            &mut self.status_info,
                            &mut browser_state.entries,
                            &mut archive_info,
                        );
                    }
                    if actions.convert_to_7z {
                        let ops_state = self.archive_operations.state_mut();
                        operations::archive::convert_archive(
                            &self.state,
                            &mut self.status_info,
                            &mut ops_state.conversion_dialog,
                            &mut ops_state.conversion_rx,
                            &mut ops_state.conversion_child,
                            &mut ops_state.conversion_started,
                        );
                    }
                    if actions.organize_archive {
                        let state = self.state.lock();
                        if let Some(archive) = &state.current_archive {
                            let archive_name = archive.file_name().unwrap_or_default().to_string_lossy().to_string();
                            drop(state);
                            
                            // Ensure rules are loaded before navigating
                            self.organization_feature.ensure_rules_loaded(&self.state);
                            
                            // Initialize panel if needed (or just let the page render handle it?)
                            // Actually, we should initialize it here so we have the data ready
                            let state = self.state.lock();
                            let entries = state.all_entries.clone();
                            let rules = self.organization_feature.rules_state.rules.clone();
                            let metadata = state.current_game_metadata.clone();
                            drop(state);

                            self.organization_feature.organize_panel = Some(crate::features::organization::OrganizePanel::new(
                                archive_name.clone(),
                                entries,
                                rules,
                                metadata,
                            ));
                            
                            self.page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                        }
                    }
                });
        }

        // Render Status Bar
        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::NONE.fill(self.theme.colors.bg_secondary))
            .show(ctx, |ui| {
                let state = self.state.lock();
                let archive_loaded = state.current_archive.is_some();
                
                // Update status info from state
                // This is a simplified mapping, ideally we'd have a dedicated update method
                if archive_loaded {
                    self.status_info.file_count = state.archive_info.file_count;
                    self.status_info.total_size = crate::core::utils::format_size(state.archive_info.total_size);
                    self.status_info.compressed_size = crate::core::utils::format_size(state.archive_info.compressed_size);
                    self.status_info.archive_format = state.archive_info.archive_format.clone();
                }
                
                let plugin_info = if let Some(manager) = &state.plugin_manager {
                    let mgr = manager.lock();
                    let list = mgr.list_plugins();
                    Some(components::status_bar::PluginStatusInfo {
                        total_plugins: list.len(),
                        enabled_plugins: list.iter().filter(|p| p.enabled).count(),
                        has_metadata: state.plugin_metadata.is_some(),
                    })
                } else {
                    None
                };
                drop(state);

                components::status_bar::render(
                    ui,
                    &self.theme,
                    &self.status_info,
                    archive_loaded,
                    plugin_info.as_ref(),
                );
            });

        // Render Password Dialog
        let shared_state = crate::shared::SharedState {
            app_state: self.state.clone(),
            theme: self.theme.clone(),
        };
        match password_management::handle_password_dialogs(
            &mut self.password_feature,
            ctx,
            &shared_state,
        ) {
            password_management::PasswordFeatureAction::PasswordUnlocked { path, password } => {
                let mut archive_info = operations::archive::ArchiveInfo::default();
                let browser_state = self.archive_browser.state_mut();
                if operations::archive::try_open_with_password(
                    &self.state,
                    &path,
                    &password,
                    &mut self.password_feature.password_dialog,
                    &mut self._pending_archive_path,
                    &mut self.status_info,
                    &mut browser_state.entries,
                    &mut archive_info,
                ) {
                    self.password_feature.password_dialog.show = false;
                    self._pending_archive_path = None;
                } else {
                    self.password_feature.password_dialog.error = "Invalid password".to_string();
                }
            }
            password_management::PasswordFeatureAction::None => {}
        }

        // Render Password Rules Dialog
        if let Some(result) = password_management::dialogs::zip_pass_rules::render_password_rules_dialog(
            ctx,
            &self.theme,
            &mut self.settings_feature.password_rules_dialog,
        ) {
            match result {
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                    self.settings_feature.password_rules_dialog.show = false;
                }
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save { rules } => {
                    self.settings_feature.handle_action(
                        crate::features::settings::settings_content::SettingsAction::SavePasswordRules { rules },
                        &crate::shared::SharedState {
                            app_state: self.state.clone(),
                            theme: self.theme.clone(),
                        },
                    );
                    self.settings_feature.password_rules_dialog.show = false;
                }
            }
        }

        // Render Extraction Progress Dialog
        if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
            ctx,
            &self.theme,
            &mut self.archive_operations.state_mut().extraction_dialog,
        ) {
            match result {
                dialogs::progress::ExtractionDialogResult::Cancelled => {
                    self.archive_operations.cancel_extraction();
                    self.archive_operations.state_mut().extraction_dialog.show = false;
                }
                dialogs::progress::ExtractionDialogResult::Minimized => {
                    self.archive_operations.state_mut().extraction_minimized = true;
                    self.archive_operations.state_mut().extraction_dialog.show = false;
                }
                dialogs::progress::ExtractionDialogResult::Paused => {
                    self.archive_operations.pause_extraction();
                }
                dialogs::progress::ExtractionDialogResult::Resumed => {
                    self.archive_operations.resume_extraction();
                }
                dialogs::progress::ExtractionDialogResult::None => {}
            }
        }

        // Render File Edit Dialog
        if let Some(result) = crate::features::file_editing::file_edit_dialog::render_file_edit_dialog(
            ctx,
            &self.theme,
            &mut self.edit_dialog,
        ) {
            match result {
                crate::features::file_editing::file_edit_dialog::FileEditResult::Save { new_name, content } => {
                    if let Some(_file) = &self._pending_edit_file {
                        let state = self.state.lock();
                        if let Some(archive) = state.current_archive.clone() {
                            match state.add_or_update_file_from_str(&archive, &new_name, &content) {
                                Ok(_) => {
                                    self.status_info.message = "File saved".to_string();
                                    // TODO: Refresh file list
                                }
                                Err(e) => {
                                    let msg = format!("Failed to save file: {}", e);
                                    crate::core::utils::log_failure("FileEdit", &msg);
                                    self.status_info.message = msg;
                                }
                            }
                        }
                    }
                    self.edit_dialog.show = false;
                }
                crate::features::file_editing::file_edit_dialog::FileEditResult::Cancel => {
                    self.edit_dialog.show = false;
                }
            }
        }

        // Render Main Content
        egui::CentralPanel::default().show(ctx, |_ui| {
            match &mut self.page_navigator.current_page {
                AppPage::Main => {
                    let action = self.archive_browser.render(
                        ctx,
                        &crate::shared::SharedState {
                            app_state: self.state.clone(),
                            theme: self.theme.clone(),
                        },
                    );
                    
                    match action {
                        crate::features::archive_browser::ArchiveBrowserAction::NavigateToFolder(folder) => {
                            crate::features::archive_browser::navigation::navigate_to_folder(
                                self.archive_browser.state_mut(),
                                &crate::shared::SharedState {
                                    app_state: self.state.clone(),
                                    theme: self.theme.clone(),
                                },
                                &folder,
                            );
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::OpenFile(file) => {
                            self.archive_operations.state_mut().pending_open_file = Some(file);
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::EditFile(file) => {
                            self._pending_edit_file = Some(file.clone());
                            self.edit_dialog.show = true;
                            self.edit_dialog.full_path_in_archive = file.clone();
                            
                            let state = self.state.lock();
                            if let Some(archive) = &state.current_archive {
                                match state.read_text_file(archive, &file) {
                                    Ok(content) => {
                                        self.edit_dialog.content = content.clone();
                                        self.edit_dialog.original_content = content;
                                    }
                                    Err(e) => {
                                        let msg = format!("Failed to read file: {}", e);
                                        crate::core::utils::log_failure("FileEdit", &msg);
                                        self.status_info.message = msg;
                                    }
                                }
                            }
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::DeleteFile(_file) => {
                            // TODO: Delete file
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::Organize => {
                            // Trigger organization flow same as toolbar
                            let state = self.state.lock();
                            if let Some(archive) = &state.current_archive {
                                let archive_name = archive.file_name().unwrap_or_default().to_string_lossy().to_string();
                                drop(state);
                                
                                self.organization_feature.ensure_rules_loaded(&self.state);
                                
                                let state = self.state.lock();
                                let entries = state.all_entries.clone();
                                let rules = self.organization_feature.rules_state.rules.clone();
                                let metadata = state.current_game_metadata.clone();
                                drop(state);
                                
                                self.organization_feature.organize_panel = Some(crate::features::organization::OrganizePanel::new(
                                    archive_name.clone(),
                                    entries,
                                    rules,
                                    metadata,
                                ));
                                
                                self.page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                            }
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::None => {}
                    }
                }
                AppPage::Settings(page) => {
                    let mut on_back = false;
                    let breadcrumb = crate::core::navigation::PageNavigator::get_breadcrumb(
                        &crate::core::AppPage::Settings(page.clone())
                    );
                    self.settings_feature.render(
                        ctx,
                        &crate::shared::SharedState {
                            app_state: self.state.clone(),
                            theme: self.theme.clone(),
                        },
                        page,
                        &mut on_back,
                        breadcrumb,
                    );

                    if on_back {
                        self.page_navigator.navigate_back();
                    }
                }
                AppPage::Plugins => {
                    self.plugins_feature.render(
                        ctx,
                        &crate::shared::SharedState {
                            app_state: self.state.clone(),
                            theme: self.theme.clone(),
                        },
                    );
                }
                AppPage::Organize => {
                    organization::rules_page::render(
                        ctx,
                        &self.theme,
                        &mut self.organization_feature.rules_state,
                        &self.state,
                    );
                }
                AppPage::OrganizeArchive(_name) => {
                    let mut action = crate::features::organization::OrganizationAction::None;
                    
                    if let Some(panel) = &mut self.organization_feature.organize_panel {
                        if let Some(result) = panel.render(ctx) {
                            if result {
                                action = crate::features::organization::OrganizationAction::Apply;
                            } else {
                                action = crate::features::organization::OrganizationAction::Cancel;
                            }
                        }
                    } else {
                        // Should not happen if navigated correctly, but handle it
                        self.page_navigator.navigate_back();
                    }

                    match action {
                        crate::features::organization::OrganizationAction::Apply => {
                            if let Some(panel) = &self.organization_feature.organize_panel {
                                if let Some(plan) = &panel.preview_plan {
                                    let shared_state = crate::shared::SharedState {
                                        app_state: self.state.clone(),
                                        theme: self.theme.clone(),
                                    };
                                    
                                    let archive_path = if let Some(state) = self.state.try_lock() {
                                        state.current_archive.clone()
                                    } else {
                                        None
                                    };

                                    if let Some(path) = archive_path {
                                        if let Err(e) = crate::features::organization::operations::execute_organization_plan(
                                            &shared_state,
                                            plan,
                                            &path,
                                            &path,
                                        ) {
                                            let msg = format!("Organization failed: {}", e);
                                            crate::core::utils::log_failure("Organization", &msg);
                                            self.status_info.message = msg;
                                        } else {
                                            self.status_info.message = "Organization completed successfully".to_string();
                                        }
                                    }
                                }
                            }
                            self.organization_feature.organize_panel = None;
                            self.page_navigator.navigate_back();
                        }
                        crate::features::organization::OrganizationAction::Cancel => {
                            self.organization_feature.organize_panel = None;
                            self.page_navigator.navigate_back();
                        }
                        crate::features::organization::OrganizationAction::None => {}
                    }
                }
            }
        });
    }
}
