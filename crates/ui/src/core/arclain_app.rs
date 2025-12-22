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
    // Core shared state (contains app_state, theme, toaster, plugin_dialog_state)
    pub(crate) shared_state: crate::shared::SharedState,

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

    // Top tab bar state
    top_tab_bar_state: components::top_tab_bar::TopTabBarState,

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
            toaster: Arc::new(parking_lot::Mutex::new(arclain_widgets::Toaster::new())),
            plugin_dialog_state: Arc::new(parking_lot::Mutex::new(plugins::PluginDialogState::new())),
            refresh_requests: Arc::new(parking_lot::Mutex::new(Vec::new())),
        };

        Self {
            shared_state: shared_state.clone(),
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
            top_tab_bar_state: components::top_tab_bar::TopTabBarState::new("archive"),
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
        // Check for pending refresh requests from plugins
        {
            let mut requests = self.shared_state.refresh_requests.lock();
            if !requests.is_empty() {
                tracing::debug!("Processing {} refresh requests: {:?}", requests.len(), requests);
                requests.clear();
                ctx.request_repaint(); // Ensure we redraw after plugin requested refresh
            }
        }
        
        // Apply theme
        self.shared_state.theme.apply_to_context(ctx);

        // Update window title
        let title = {
            let state = self.shared_state.app_state.lock();
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
        
        // Process pending file opens (double-click on file in archive)
        if let Some(file_path) = self.archive_operations.state_mut().pending_open_file.take() {
            if let Some(nested_archive_path) = crate::features::archive_operations::open_file_from_archive(
                &self.shared_state.app_state,
                &file_path,
                &mut self.status_info,
            ) {
                // It's a nested archive - open it as the current archive
                let browser_state = self.archive_browser.state_mut();
                let mut archive_info = operations::archive::ArchiveInfo::default();
                operations::archive::open_archive_by_path(
                    &self.shared_state.app_state,
                    &nested_archive_path,
                    &mut browser_state.current_path,
                    &mut self.password_feature.password_dialog,
                    &mut self.status_info,
                    &mut browser_state.entries,
                    &mut archive_info,
                );
            }
        }

        // Render Header
        egui::TopBottomPanel::top("header_panel")
            .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface_variant))
            .show(ctx, |ui| {
                let mut theme_toggle = false;
                let can_go_back = self.page_navigator.can_go_back();
                let is_on_settings = self.page_navigator.is_on_settings();
                
                // Sync UI preferences from AppState
                {
                    let state = self.shared_state.app_state.lock();
                    self.header_state.show_button_labels = state.ui_preferences.show_button_labels;
                }
                
                let actions = components::header::render(
                    ui,
                    &self.shared_state.theme,
                    &mut self.header_state,
                    &mut theme_toggle,
                    true, // Always show nav buttons for now
                    can_go_back,
                    is_on_settings,
                );

                if theme_toggle {
                    self.shared_state.theme.toggle();
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

        // Render Top Tab Bar
        egui::TopBottomPanel::top("top_tab_bar_panel")
            .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface))
            .show(ctx, |ui| {
                // Build combined tabs list: host tabs + plugin tabs
                let mut tabs = vec![
                    components::top_tab_bar::TopTab {
                        id: "archive".to_string(),
                        label: "Archive".to_string(),
                        icon: egui_phosphor::regular::FOLDER_OPEN.to_string(),
                        badge: None,
                        source: None,
                    },
                ];

                // Collect plugin tabs
                {
                    let state = self.shared_state.app_state.lock();
                    if let Some(plugin_manager) = &state.plugin_manager {
                        if let Some(pm) = plugin_manager.try_lock() {
                            for (plugin_id, tab_config) in pm.get_all_top_tabs() {
                                tabs.push(components::top_tab_bar::TopTab {
                                    id: tab_config.id.clone(),
                                    label: tab_config.label,
                                    icon: tab_config.icon,
                                    badge: tab_config.badge,
                                    source: Some(plugin_id),
                                });
                            }
                        }
                    }
                }

                // Note: Settings tab removed - already accessible via header button

                // Render tab bar and handle actions
                if let Some(action) = components::top_tab_bar::render(
                    ui,
                    &self.shared_state.theme.colors,
                    &mut self.top_tab_bar_state,
                    &tabs,
                ) {
                    match action {
                        components::top_tab_bar::TopTabAction::SelectHostTab(id) => {
                            match id.as_str() {
                                "archive" => {
                                    // Close any open plugin pages first
                                    {
                                        let mut dialog_state = self.shared_state.plugin_dialog_state.lock();
                                        dialog_state.page_stack.clear();
                                    }
                                    // Navigate to main without adding to history
                                    self.page_navigator.navigate_to_main();
                                }
                                _ => {}
                            }
                        }
                        components::top_tab_bar::TopTabAction::SelectPluginTab { plugin_id, tab_id } => {
                            // Open plugin page for the selected tab
                            // Clear existing pages first to avoid stacking
                            let mut dialog_state = self.shared_state.plugin_dialog_state.lock();
                            dialog_state.page_stack.clear();
                            dialog_state.open_page(&plugin_id, &tab_id);
                        }
                    }
                }
            });

        // Render Toolbar (only on Main page)
        if self.page_navigator.is_on_main() {
            egui::TopBottomPanel::top("toolbar_panel")
                .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface_variant))
                .show(ctx, |ui| {
                    let state = self.shared_state.app_state.lock();
                    let can_go_back = state.navigation.can_go_back();
                    let can_go_forward = state.navigation.can_go_forward();
                    let can_go_up = state.navigation.can_go_up();
                    let archive_loaded = state.current_archive.is_some();
                    let has_selection = false; // TODO: Implement selection tracking
                    let has_metadata = state.plugin_metadata.is_some();
                    let toolbar_config = components::toolbar::ToolbarConfig::new(state.toolbar_items.clone());
                    let plugin_manager = state.plugin_manager.clone();
                    drop(state);

                    let actions = components::toolbar::render(
                        ui,
                        &self.shared_state.theme,
                        &mut self.archive_browser.state_mut().toolbar_state,
                        can_go_back,
                        can_go_forward,
                        can_go_up,
                        archive_loaded,
                        has_selection,
                        has_metadata,
                        Some(&toolbar_config),
                        plugin_manager.as_ref(),
                        Some(&self.shared_state),
                    );

                    // Handle toolbar actions
                    let shared_state = self.shared_state.clone();

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
                            &self.shared_state.app_state,
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
                            &self.shared_state.app_state,
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
                            &self.shared_state.app_state,
                            &mut ops_state.extraction_dialog,
                            &mut ops_state.extraction_rx,
                            &mut ops_state.extraction_child,
                            &mut ops_state.extraction_minimized,
                            &mut ops_state.extraction_started,
                            &mut self.status_info,
                        );
                    }
                    if actions.add {
                        operations::file::add_files(&self.shared_state.app_state, &mut self.status_info);
                    }
                    if actions.delete_selected {
                        let mut archive_info = operations::archive::ArchiveInfo::default();
                        let browser_state = self.archive_browser.state_mut();
                        let entries_clone = browser_state.entries.clone();
                        operations::file::delete_selected(
                            &self.shared_state.app_state,
                            &entries_clone,
                            &mut self.status_info,
                            &mut browser_state.entries,
                            &mut archive_info,
                        );
                    }
                    if actions.convert_to_7z {
                        let ops_state = self.archive_operations.state_mut();
                        operations::archive::convert_archive(
                            &self.shared_state.app_state,
                            &mut self.status_info,
                            &mut ops_state.conversion_dialog,
                            &mut ops_state.conversion_rx,
                            &mut ops_state.conversion_child,
                            &mut ops_state.conversion_started,
                        );
                    }
                    if actions.organize_archive {
                        let state = self.shared_state.app_state.lock();
                        if let Some(archive) = &state.current_archive {
                            let archive_name = archive.file_name().unwrap_or_default().to_string_lossy().to_string();
                            drop(state);
                            
                            // Load rules directly from DB and filter by enabled plugins
                            let mut rules = Vec::new(); // Default empty
                            {
                                let state = self.shared_state.app_state.lock();
                                
                                // Check enabled plugins (specifically DLsite)
                                let dlsite_enabled = if let Some(manager) = &state.plugin_manager {
                                    let mgr = manager.lock();
                                    mgr.list_plugins().iter().any(|p| p.id.eq_ignore_ascii_case("dlsite-metadata") && p.enabled)
                                } else {
                                    false
                                };


                                if let Some(dbs) = &state.dbs {
                                   let db = &dbs.config;
                                   if let Ok(loaded) = arclain_core::config::database::list_org_rules(db) {
                                       rules = loaded.into_iter().filter(|r| {
                                            if r.trigger.metadata_source.as_deref().map(|s| s.eq_ignore_ascii_case("dlsite")).unwrap_or(false) {
                                                dlsite_enabled
                                            } else {
                                                true
                                            }
                                       }).collect();
                                   }
                                }
                            }
                            
                            // Initialize panel
                            let state = self.shared_state.app_state.lock();
                            let entries = state.all_entries.clone();
                            let metadata = state.current_game_metadata.clone();
                            drop(state);

                            self.organization_feature.organizer_page = Some(crate::features::organization::OrganizerPage::new(
                                crate::features::organization::OrganizePanel::new(
                                    archive_name.clone(),
                                    entries,
                                    rules,
                                    metadata,
                                )
                            ));
                            
                            self.page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                        }
                    }
                });
        }

        // Render Status Bar
        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface_variant))
            .show(ctx, |ui| {
                let state = self.shared_state.app_state.lock();
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
                    &self.shared_state.theme,
                    &self.status_info,
                    archive_loaded,
                    plugin_info.as_ref(),
                );
            });

        // Render Password Dialog
        let shared_state = self.shared_state.clone();
        match password_management::handle_password_dialogs(
            &mut self.password_feature,
            ctx,
            &shared_state,
        ) {
            password_management::PasswordFeatureAction::PasswordUnlocked { path, password } => {
                let mut archive_info = operations::archive::ArchiveInfo::default();
                let browser_state = self.archive_browser.state_mut();
                if operations::archive::try_open_with_password(
                    &self.shared_state.app_state,
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
            &self.shared_state.theme,
            &mut self.settings_feature.password_rules_dialog,
        ) {
            match result {
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                    self.settings_feature.password_rules_dialog.show = false;
                }
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save { rules } => {
                    self.settings_feature.handle_action(
                        crate::features::settings::settings_content::SettingsAction::SavePasswordRules { rules },
                        &self.shared_state,
                    );
                    self.settings_feature.password_rules_dialog.show = false;
                }
            }
        }

        // Render Extraction Progress Dialog
        if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
            ctx,
            &self.shared_state.theme,
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
            &self.shared_state.theme,
            &mut self.edit_dialog,
        ) {
            match result {
                crate::features::file_editing::file_edit_dialog::FileEditResult::Save { new_name, content } => {
                    if let Some(_file) = &self._pending_edit_file {
                        let state = self.shared_state.app_state.lock();
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

        // Check for plugin page first - if open, render it instead of normal content
        if self.render_plugin_page(ctx) {
            // Plugin page handled content, skip normal rendering
            // But still need to:
        } else {
            // Render Main Content
            egui::CentralPanel::default().show(ctx, |_ui| {
                let current_page = self.page_navigator.current_page.clone();
            match current_page {
                AppPage::Main => {

                    let shared_state = self.shared_state.clone();

                    let action = self.archive_browser.render(
                        ctx,
                        &shared_state,
                    );
                    
                    match action {
                        crate::features::archive_browser::ArchiveBrowserAction::NavigateToFolder(
                            folder,
                        ) => {
                            crate::features::archive_browser::navigation::navigate_to_folder(
                                self.archive_browser.state_mut(),
                                &shared_state,
                                &folder,
                            );
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::NavigateToPath(
                            path,
                        ) => {
                            crate::features::archive_browser::navigation::navigate_to_path(
                                self.archive_browser.state_mut(),
                                &shared_state,
                                &path,
                            );
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::OpenFile(file) => {
                            self.archive_operations.state_mut().pending_open_file = Some(file);
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::OpenArchiveInTab(archive_path) => {
                            // Extract nested archive to temp and open as current archive
                            if let Some(extracted_path) = crate::features::archive_operations::open_file_from_archive(
                                &self.shared_state.app_state,
                                &archive_path,
                                &mut self.status_info,
                            ) {
                                // Open the extracted archive as the current archive
                                let browser_state = self.archive_browser.state_mut();
                                let mut archive_info = operations::archive::ArchiveInfo::default();
                                operations::archive::open_archive_by_path(
                                    &self.shared_state.app_state,
                                    &extracted_path,
                                    &mut browser_state.current_path,
                                    &mut self.password_feature.password_dialog,
                                    &mut self.status_info,
                                    &mut browser_state.entries,
                                    &mut archive_info,
                                );
                            }
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::EditFile(file) => {
                            self._pending_edit_file = Some(file.clone());
                            self.edit_dialog.show = true;
                            self.edit_dialog.full_path_in_archive = file.clone();
                            
                            let state = self.shared_state.app_state.lock();
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
                        crate::features::archive_browser::ArchiveBrowserAction::Metadata(json) => {
                            // Parse metadata JSON and store in state
                            match serde_json::from_str::<arclain_core::features::organization::GameMetadata>(&json) {
                                Ok(metadata) => {
                                    tracing::info!("Received metadata from plugin: {:?}", metadata.title);
                                    let mut state = self.shared_state.app_state.lock();
                                    state.current_game_metadata = Some(metadata);
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse metadata JSON: {}", e);
                                }
                            }
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::Organize => {
                            // Trigger organization flow same as toolbar
                            let state = self.shared_state.app_state.lock();
                            if let Some(archive) = &state.current_archive {
                                let archive_name = archive.file_name().unwrap_or_default().to_string_lossy().to_string();
                                drop(state);
                                
                                // Load rules directly from DB and filter by enabled plugins
                                let mut rules = Vec::new(); // Default empty
                                {
                                    let state = self.shared_state.app_state.lock();
                                    
                                    // Check enabled plugins (specifically DLsite)
                                    let dlsite_enabled = if let Some(manager) = &state.plugin_manager {
                                        let mgr = manager.lock();
                                        mgr.list_plugins().iter().any(|p| p.id.eq_ignore_ascii_case("dlsite") && p.enabled)
                                    } else {
                                        false
                                    };

                                    if let Some(dbs) = &state.dbs {
                                       let db = &dbs.config;
                                       if let Ok(loaded) = arclain_core::config::database::list_org_rules(db) {
                                           rules = loaded.into_iter().filter(|r| {
                                                if r.trigger.metadata_source.as_deref().map(|s| s.eq_ignore_ascii_case("dlsite")).unwrap_or(false) {
                                                    dlsite_enabled
                                                } else {
                                                    true
                                                }
                                           }).collect();
                                       }
                                    }
                                }
                                
                                let state = self.shared_state.app_state.lock();
                                let entries = state.all_entries.clone();
                                let metadata = state.current_game_metadata.clone();
                                drop(state);
                                
                                self.organization_feature.organizer_page = Some(crate::features::organization::OrganizerPage::new(
                                    crate::features::organization::OrganizePanel::new(
                                        archive_name.clone(),
                                        entries,
                                        rules,
                                        metadata,
                                    )
                                ));
                                
                                self.page_navigator.navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                            }
                        }
                        // Context menu actions
                        crate::features::archive_browser::ArchiveBrowserAction::Extract(file) => {
                            // Extract single file to default location
                            let _browser_state = self.archive_browser.state_mut();
                            let ops_state = self.archive_operations.state_mut();
                            // Create a temporary selection for just this file
                            let entries: Vec<crate::shared::components::file_list::FileEntry> = vec![
                                crate::shared::components::file_list::FileEntry {
                                    name: file,
                                    selected: true,
                                    size: String::new(),
                                    compressed: String::new(),
                                    ratio: String::new(),
                                    modified: String::new(),
                                    crc32: String::new(),
                                    encrypted: false,
                                    is_folder: false,
                                }
                            ];
                            operations::extraction::extract_selected(
                                &self.shared_state.app_state,
                                &entries,
                                &mut ops_state.extraction_dialog,
                                &mut ops_state.extraction_rx,
                                &mut ops_state.extraction_child,
                                &mut ops_state.extraction_minimized,
                                &mut ops_state.extraction_started,
                                &mut self.status_info,
                            );
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::ExtractTo(file) => {
                            // TODO: Show folder picker dialog then extract
                            self.status_info.message = format!("Extract to... for '{}' - not yet implemented", file);
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::CopyPath(file) => {
                            // Copy file path to clipboard
                            let state = self.shared_state.app_state.lock();
                            let full_path = if state.navigation.current_path.is_empty() {
                                file.clone()
                            } else {
                                format!("{}/{}", state.navigation.current_path, file)
                            };
                            drop(state);
                            ctx.copy_text(full_path.clone());
                            self.status_info.message = format!("Copied path: {}", full_path);
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::ShowProperties(file) => {
                            // Enable properties panel and select only this file
                            self.archive_browser.state_mut().toolbar_state.show_properties_panel = true;
                            for entry in &mut self.archive_browser.state_mut().entries {
                                entry.selected = entry.name == file;
                            }
                        }
                        crate::features::archive_browser::ArchiveBrowserAction::None => {}
                    }
                }
                AppPage::Settings(page) => {
                    let breadcrumb = crate::core::navigation::PageNavigator::get_breadcrumb(
                        &crate::core::AppPage::Settings(page.clone())
                    );
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if let Some(target) = self.settings_feature.render(
                            ui,
                            &self.shared_state,
                            &page,
                            breadcrumb,
                            Some(&mut self.organization_feature.rules_page),
                            &self.header_state.search_text,
                        ) {
                            self.page_navigator.navigate_to(target);
                        }
                    });
                }
                AppPage::Plugins => {
                    self.plugins_feature.render(
                        ctx,
                        &self.shared_state,
                    );
                }
                AppPage::Organize => {
                    egui::CentralPanel::default().show(ctx, |ui| {
                         // Extract DB (generic way, minimal lock)
                         let db_opt = {
                             let state = self.shared_state.app_state.lock();
                             if let Some(dbs) = &state.dbs {
                                 Some(dbs.config.clone()) // Clone ConfigDb (cheap Arc)
                             } else {
                                 None
                             }
                         };

                         if let Some(cfg_db) = db_opt {
                             self.organization_feature.rules_page.render(ui, &self.shared_state.theme, &cfg_db);
                         } else {
                             ui.label("Database not available.");
                         }
                    });
                }
                AppPage::OrganizeArchive(_name) => {
                    let shared_state = self.shared_state.clone();
                    let action = self.organization_feature.render(ctx, &shared_state);

                    match action {
                        crate::features::organization::OrganizationAction::Apply => {
                            if let Some(page) = &self.organization_feature.organizer_page {
                                if let Some(plan) = &page.panel.session.preview_plan {
                                    let shared_state = self.shared_state.clone();
                                    
                                    let archive_path = if let Some(state) = self.shared_state.app_state.try_lock() {
                                        state.current_archive.clone()
                                    } else {
                                        None
                                    };

                                    if let Some(path) = archive_path {
                                        // Build destination path by changing extension to .7z
                                        let dest_path = path.with_extension("7z");
                                        
                                        if let Err(e) = crate::features::organization::operations::execute_organization_plan(
                                            &shared_state,
                                            plan,
                                            &path,
                                            &dest_path,
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
                            self.organization_feature.organizer_page = None;
                            self.page_navigator.navigate_back();
                        }
                        crate::features::organization::OrganizationAction::ManageRules => {
                            self.page_navigator.navigate_to(crate::core::AppPage::Settings(crate::core::SettingsPage::OrganizationRules));
                        }
                        crate::features::organization::OrganizationAction::None => {}
                    }
                }
            }
        });
        } // Close else block for plugin page check
        
        // Render toast notifications (always on top)
        self.shared_state.toaster.lock().show(ctx);
        
        // Render plugin dialog if open
        self.render_plugin_dialog(ctx);
    }
}

impl ArclainApp {
    /// Render an open plugin dialog as a modal overlay
    fn render_plugin_dialog(&mut self, ctx: &egui::Context) {
        // Check if a dialog is open (get info before locking for rendering)
        let dialog_info = {
            let dialog_state = self.shared_state.plugin_dialog_state.lock();
            dialog_state.open_dialog.clone()
        };
        
        if let Some((plugin_id, dialog_id)) = dialog_info {
            // Get dialog UI elements from plugin
            let dialog_elements = {
                let state = self.shared_state.app_state.lock();
                if let Some(pm_arc) = &state.plugin_manager {
                    let pm = pm_arc.lock();
                    pm.with_plugin_instance(&plugin_id, |instance| {
                        instance.get_ui_layout(arclain_plugins::types::PluginExtensionPoint::Dialog(dialog_id.clone()))
                            .unwrap_or_default()
                    })
                    .unwrap_or_default()
                } else {
                    arclain_plugins::types::PluginLayout::default()
                }
            };
            
            // Render modal dialog
            let mut open = true;
            egui::Window::new(format!("Plugin Dialog - {}", dialog_id))
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([400.0, 300.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    // Set up callback for dialog events
                    let state_arc = self.shared_state.app_state.clone();
                    let pid = plugin_id.clone();
                    let dialog_state_arc = self.shared_state.plugin_dialog_state.clone();
                    let toaster_arc = self.shared_state.toaster.clone();
                    
                    let mut callback: Box<dyn FnMut(&str, Option<String>)> = 
                        Box::new(move |element_id: &str, value: Option<String>| {
                            // Check for close dialog signal
                            if element_id == "__dialog_close" {
                                dialog_state_arc.lock().close_dialog();
                                return;
                            }
                            
                            // Normal event
                            let state = state_arc.lock();
                            if let Some(pm_arc) = &state.plugin_manager {
                                let pm = pm_arc.lock();
                                if let Some(actions) = pm
                                    .with_plugin_instance(&pid, |instance| {
                                        instance.send_ui_event(element_id, value).ok()
                                    })
                                    .flatten()
                                {
                                    drop(pm); // Release plugin manager lock before locking toaster
                                    let mut toaster = toaster_arc.lock();
                                    let mut ds = dialog_state_arc.lock();
                                    for action in actions {
                                        crate::features::plugins::action_handler::process_plugin_actions(
                                            vec![action],
                                            &pid,
                                            &mut ds,
                                            &mut toaster,
                                            None, // No refresh requests for dialog callbacks
                                        );
                                    }
                                }
                            }
                        });
                    
                    let flat_elements = dialog_elements.flatten();
                    crate::features::plugins::plugin_ui::render_ui_elements(
                        ui,
                        &flat_elements,
                        &mut callback,
                        &self.shared_state.theme.colors,
                        None,
                    );
                });
            
            // If window was closed via X button
            if !open {
                self.shared_state.plugin_dialog_state.lock().close_dialog();
            }
        }
    }
    
    /// Render an open plugin page (replaces main content area)
    /// Returns true if a page is being rendered (caller should skip normal content)
    fn render_plugin_page(&mut self, ctx: &egui::Context) -> bool {
        // Check if a page is open
        let page_info = {
            let dialog_state = self.shared_state.plugin_dialog_state.lock();
            dialog_state.current_page().map(|(p, d)| (p.to_string(), d.to_string()))
        };
        
        let Some((plugin_id, page_id)) = page_info else {
            return false;
        };
        
        // Get page UI layout from plugin
        let page_layout = {
            let state = self.shared_state.app_state.lock();
            if let Some(pm_arc) = &state.plugin_manager {
                let pm = pm_arc.lock();
                pm.with_plugin_instance(&plugin_id, |instance| {
                    instance.get_ui_layout(arclain_plugins::types::PluginExtensionPoint::Page(page_id.clone()))
                        .unwrap_or_default()
                })
                .unwrap_or_default()
            } else {
                arclain_plugins::types::PluginLayout::default()
            }
        };
        
        // Render as full page content
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface))
            .show(ctx, |ui| {
                // Back button at top
                ui.horizontal(|ui| {
                    if ui.button("← Back").clicked() {
                        self.shared_state.plugin_dialog_state.lock().close_page();
                    }
                    ui.label(egui::RichText::new(&page_id).strong());
                });
                ui.separator();
                
                // Set up callback for page events
                let state_arc = self.shared_state.app_state.clone();
                let pid = plugin_id.clone();
                let dialog_state_arc = self.shared_state.plugin_dialog_state.clone();
                let toaster_arc = self.shared_state.toaster.clone();
                
                let mut callback: Box<dyn FnMut(&str, Option<String>)> = 
                    Box::new(move |element_id: &str, value: Option<String>| {
                        // Check for close page signal
                        if element_id == "__page_close" {
                            dialog_state_arc.lock().close_page();
                            return;
                        }
                        
                        // Check for open page signal (nested navigation)
                        if element_id.starts_with("__page_open:") {
                            let new_page_id = element_id.trim_start_matches("__page_open:").to_string();
                            dialog_state_arc.lock().open_page(&pid, &new_page_id);
                            return;
                        }
                        
                        // Normal event
                        let state = state_arc.lock();
                        if let Some(pm_arc) = &state.plugin_manager {
                            let pm = pm_arc.lock();
                            if let Some(actions) = pm
                                .with_plugin_instance(&pid, |instance| {
                                    instance.send_ui_event(element_id, value).ok()
                                })
                                .flatten()
                            {
                                drop(pm);
                                let mut toaster = toaster_arc.lock();
                                let mut ds = dialog_state_arc.lock();
                                for action in actions {
                                    crate::features::plugins::action_handler::process_plugin_actions(
                                        vec![action],
                                        &pid,
                                        &mut ds,
                                        &mut toaster,
                                        None, // No refresh requests for page callbacks
                                    );
                                }
                            }
                        }
                    });
                
                use arclain_plugins::types::PluginLayout;
                match page_layout {
                    PluginLayout::Single { elements } => {
                        crate::features::plugins::plugin_ui::render_ui_elements(
                            ui,
                            &elements,
                            &mut callback,
                            &self.shared_state.theme.colors,
                            None,
                        );
                    }
                    PluginLayout::Split { sidebar, content, sidebar_width } => {
                       egui::SidePanel::left(format!("plugin_split_sidebar_{}", page_id))
                            .resizable(true)
                            .default_width(sidebar_width.unwrap_or(250.0))
                            .show_inside(ui, |ui| {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    crate::features::plugins::plugin_ui::render_ui_elements(
                                        ui,
                                        &sidebar,
                                        &mut callback,
                                        &self.shared_state.theme.colors,
                                        None,
                                    );
                                });
                            });
                        
                        egui::CentralPanel::default().show_inside(ui, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                crate::features::plugins::plugin_ui::render_ui_elements(
                                    ui,
                                    &content,
                                    &mut callback,
                                    &self.shared_state.theme.colors,
                                    None,
                                );
                            });
                        });
                    }
                }
            });
        
        true
    }
}

