//! Main application coordinator
//!
//! The ArclainApp struct serves as the primary coordination point for the entire UI,
//! managing global state and delegating rendering to feature modules.

use crate::core::{
    navigation::{AppPage, PageNavigator, SettingsPage},
    operations,
    // state::AppState,
};
use crate::features::{organization, password_management, plugins, settings};
// use crate::platform::detect_dark_mode;
use crate::shared::{components, dialogs};

use eframe::egui;
// use parking_lot::Mutex;
use std::path::PathBuf;
// use std::sync::Arc;

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
    _signals_bound: bool,
    show_log_viewer: bool,
}

impl ArclainApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Initialize shared state (includes AppState, Services, Theme)
        let shared_state = crate::shared::SharedState::new(cc);

        Self {
            shared_state: shared_state.clone(),
            page_navigator: PageNavigator::new(),
            header_state: components::HeaderState::default(),
            archive_browser: crate::features::archive_browser::ArchiveBrowser::new(&shared_state),
            archive_operations: crate::features::archive_operations::ArchiveOperations::new(
                &shared_state,
            ),
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
            // _pending_open_file: None,
            // archive_info: operations::archive::ArchiveInfo::default(),
            _last_window_title: None,
            // extraction_dialog: dialogs::progress::ExtractionProgressDialog::default(),
            // _extraction_rx: None,
            // _extraction_child: None,
            // extraction_minimized: false,
            // _extraction_started: None,
            _password_rules_loaded: false,
            _signals_bound: false,
            show_log_viewer: false,
        }
    }
}

impl eframe::App for ArclainApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use crate::core::{app_lifecycle, app_rendering};

        // === Lifecycle: Refresh requests, signals, theme ===
        app_lifecycle::process_refresh_requests(&self.shared_state, ctx);
        app_lifecycle::bind_signals_once(
            &self.shared_state.app_state,
            ctx,
            &mut self._signals_bound,
        );
        app_lifecycle::apply_theme(&self.shared_state, ctx);

        // === Lifecycle: Process metadata signal updates from plugins ===
        app_lifecycle::process_metadata_signal(&self.shared_state, &mut self.organization_feature);

        // === Lifecycle: Handle extraction progress from native backends ===
        {
            let ops_state = self.archive_operations.state_mut();
            app_lifecycle::process_extraction_progress(
                &self.shared_state,
                &mut ops_state.extraction_dialog,
                &mut self.status_info.message,
                ctx,
            );
        }

        // === Lifecycle: Update window title ===
        app_lifecycle::update_window_title(
            &self.shared_state,
            &self.page_navigator,
            &mut self._last_window_title,
            ctx,
        );

        // Handle extraction/conversion progress from CLI backends
        self.archive_operations.update_extraction_progress(ctx);
        self.archive_operations.update_conversion_progress(ctx);

        // Process pending file opens (double-click on file in archive)
        if let Some(file_path) = self.archive_operations.state_mut().pending_open_file.take() {
            if let Some(nested_archive_path) =
                crate::features::archive_operations::open_file_from_archive(
                    &self.shared_state.app_state,
                    &file_path,
                    &mut self.status_info,
                )
            {
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

        // === Render Header Panel ===
        let header_actions = app_rendering::render_header_panel(
            ctx,
            &self.shared_state,
            &self.page_navigator,
            &mut self.header_state,
        );

        // Handle header actions
        if header_actions.theme_toggle {
            self.shared_state.theme.toggle();
        }
        if header_actions.navigate_home {
            self.page_navigator.navigate_to_main();
        }
        if header_actions.navigate_back {
            self.page_navigator.navigate_back();
        }
        if header_actions.navigate_plugins {
            self.page_navigator.navigate_to(AppPage::Plugins);
        }
        if header_actions.navigate_settings {
            self.page_navigator
                .navigate_to(AppPage::Settings(SettingsPage::Overview));
        }
        if header_actions.show_logs {
            self.show_log_viewer = true;
        }

        // === Render Tab Bar Panel ===
        let tab_action = app_rendering::render_tab_bar_panel(
            ctx,
            &self.shared_state,
            &mut self.top_tab_bar_state,
        );

        // Handle tab bar actions
        match tab_action {
            app_rendering::TabBarAction::SelectArchiveTab => {
                // Set toolbar context to Archive
                {
                    let state = self.shared_state.app_state.lock();
                    state
                        .signals
                        .active_toolbar
                        .set(crate::core::signals::ToolbarContext::Archive);
                    state.signals.status_message.set(None);
                }
                // Close any open plugin pages
                {
                    let mut dialog_state = self.shared_state.plugin_dialog_state.lock();
                    dialog_state.page_stack.clear();
                }
                self.page_navigator.navigate_to_main();
            }
            app_rendering::TabBarAction::SelectPluginTab { plugin_id, tab_id } => {
                // Set toolbar context to Plugin
                {
                    let state = self.shared_state.app_state.lock();
                    state
                        .signals
                        .active_toolbar
                        .set(crate::core::signals::ToolbarContext::Plugin(
                            plugin_id.clone(),
                        ));
                }
                // Open plugin page
                let mut dialog_state = self.shared_state.plugin_dialog_state.lock();
                dialog_state.page_stack.clear();
                dialog_state.open_page(&plugin_id, &tab_id);
            }
            app_rendering::TabBarAction::None => {}
        }

        // Render Toolbar (only on Main page AND when Archive context is active)
        let should_show_archive_toolbar = if self.page_navigator.is_on_main() {
            let state = self.shared_state.app_state.lock();
            matches!(
                state.signals.active_toolbar.get(),
                crate::core::signals::ToolbarContext::Archive
            )
        } else {
            false
        };

        if should_show_archive_toolbar {
            egui::TopBottomPanel::top("toolbar_panel")
                .frame(egui::Frame::NONE.fill(self.shared_state.theme.colors.surface_variant))
                .show(ctx, |ui| {
                    let state = self.shared_state.app_state.lock();
                    let nav = state.signals.navigation.get();
                    let can_go_back = nav.can_go_back();
                    let can_go_forward = nav.can_go_forward();
                    let can_go_up = nav.can_go_up();
                    let archive_loaded = state.signals.archive_path.get().is_some();
                    let has_selection = false; // TODO: Implement selection tracking
                    let has_metadata = state.signals.metadata.get().is_some();
                    let toolbar_config =
                        components::toolbar::ToolbarConfig::new(state.signals.toolbar_items.get());
                    drop(state);
                    let plugin_manager = self.shared_state.services.plugin_manager.clone();

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
                        operations::file::add_files(
                            &self.shared_state.app_state,
                            &mut self.status_info,
                        );
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
                        if let Some(archive) = state.signals.archive_path.get() {
                            let archive_name = archive
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            drop(state);

                            // Load rules directly from DB and filter by enabled plugins
                            let mut rules = Vec::new(); // Default empty
                            {
                                // Check enabled plugins (specifically DLsite) from services
                                let dlsite_enabled = if let Some(manager) =
                                    &self.shared_state.services.plugin_manager
                                {
                                    let mgr = manager.lock();
                                    mgr.list_plugins().iter().any(|p| {
                                        p.id.eq_ignore_ascii_case("dlsite-metadata") && p.enabled
                                    })
                                } else {
                                    false
                                };

                                let state = self.shared_state.app_state.lock();

                                if let Some(dbs) = &state.dbs {
                                    let pool = &dbs.config_pool;
                                    if let Ok(loaded) =
                                        arclain_core::config::database::list_org_rules(pool)
                                    {
                                        rules = loaded
                                            .into_iter()
                                            .filter(|r| {
                                                if r.trigger
                                                    .metadata_source
                                                    .as_deref()
                                                    .map(|s| s.eq_ignore_ascii_case("dlsite"))
                                                    .unwrap_or(false)
                                                {
                                                    dlsite_enabled
                                                } else {
                                                    true
                                                }
                                            })
                                            .collect();
                                    }
                                }
                            }

                            // Initialize panel
                            let state = self.shared_state.app_state.lock();
                            let entries = state.signals.entries.get().as_ref().clone();
                            let metadata = state.signals.game_metadata.get();
                            drop(state);

                            self.organization_feature.organizer_page =
                                Some(crate::features::organization::OrganizerPage::new(
                                    crate::features::organization::OrganizePanel::new(
                                        archive_name.clone(),
                                        entries,
                                        rules,
                                        metadata,
                                    ),
                                ));

                            self.page_navigator
                                .navigate_to(crate::core::AppPage::OrganizeArchive(archive_name));
                        }
                    }
                });
        }

        // === Render Status Bar ===
        app_rendering::render_status_bar_panel(ctx, &self.shared_state, &mut self.status_info);

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
        if let Some(result) =
            password_management::dialogs::zip_pass_rules::render_password_rules_dialog(
                ctx,
                &self.shared_state.theme,
                &mut self.settings_feature.password_rules_dialog,
            )
        {
            match result {
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                    self.settings_feature.password_rules_dialog.show = false;
                }
                password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save {
                    rules,
                } => {
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
                    // Set signal-based cancellation for native backends
                    {
                        let state = self.shared_state.app_state.lock();
                        state
                            .signals
                            .extraction_cancel
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        state.signals.extraction_progress.set(None);
                    }
                    // Also cancel CLI extraction if any
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
        if let Some(result) =
            crate::features::file_editing::file_edit_dialog::render_file_edit_dialog(
                ctx,
                &self.shared_state.theme,
                &mut self.edit_dialog,
            )
        {
            match result {
                crate::features::file_editing::file_edit_dialog::FileEditResult::Save {
                    new_name,
                    content,
                } => {
                    let state = self.shared_state.app_state.lock();
                    if let Some(archive) = state.signals.archive_path.get() {
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

                        let action = self.archive_browser.render(ctx, &shared_state);

                        // Handle actions via ActionContext
                        {
                            let mut action_ctx = crate::features::archive_browser::ActionContext {
                                shared: &shared_state,
                                browser_state: self.archive_browser.state_mut(),
                                archive_ops_state: self.archive_operations.state_mut(),
                                status_info: &mut self.status_info,
                                password_dialog: &mut self.password_feature.password_dialog,
                                edit_dialog: &mut self.edit_dialog,
                                organization_feature: &mut self.organization_feature,
                                page_navigator: &mut self.page_navigator,
                                egui_ctx: ctx,
                            };
                            if action_ctx.handle_navigation(&action)
                                || action_ctx.handle_simple(&action)
                                || action_ctx.handle_complex(&action)
                            {
                                // Action handled
                            }
                        }
                    }
                    AppPage::Settings(page) => {
                        let breadcrumb = crate::core::navigation::PageNavigator::get_breadcrumb(
                            &crate::core::AppPage::Settings(page.clone()),
                        );
                        // Get search_text from signal
                        let search_text = {
                            let state = self.shared_state.app_state.lock();
                            state.signals.search_text.get()
                        };
                        egui::CentralPanel::default().show(ctx, |ui| {
                            if let Some(target) = self.settings_feature.render(
                                ui,
                                &self.shared_state,
                                &page,
                                breadcrumb,
                                Some(&mut self.organization_feature.rules_page),
                                &search_text,
                            ) {
                                self.page_navigator.navigate_to(target);
                            }
                        });
                    }
                    AppPage::Plugins => {
                        self.plugins_feature.render(ctx, &self.shared_state);
                    }
                    AppPage::Organize => {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            // Get OrganizationService from Services container
                            if let Some(org_service) =
                                self.shared_state.services.organization_service.as_ref()
                            {
                                self.organization_feature.rules_page.render(
                                    ui,
                                    &self.shared_state.theme,
                                    org_service,
                                );
                            } else {
                                ui.label("Organization service not available.");
                            }
                        });
                    }
                    AppPage::OrganizeArchive(_name) => {
                        let shared_state = self.shared_state.clone();
                        let action = self.organization_feature.render(ctx, &shared_state);

                        let mut action_ctx =
                            crate::features::organization::actions::ActionContext {
                                shared: &shared_state,
                                organization_feature: &mut self.organization_feature,
                                page_navigator: &mut self.page_navigator,
                                status_info: &mut self.status_info,
                            };
                        action_ctx.handle(&action);
                    }
                }
            });
        } // Close else block for plugin page check

        // Render toast notifications (always on top)
        self.shared_state.toaster.lock().show(ctx);

        // Render plugin dialog if open
        self.render_plugin_dialog(ctx);

        // Render log viewer modal if open
        if self.show_log_viewer {
            let logs = if let Some(manager) = &self.shared_state.services.plugin_manager {
                manager.lock().get_network_log()
            } else {
                Vec::new()
            };
            dialogs::log_viewer::render(
                ctx,
                &self.shared_state.theme,
                &logs,
                &mut self.show_log_viewer,
            );
        }
    }
}

impl ArclainApp {
    /// Render an open plugin dialog as a modal overlay
    fn render_plugin_dialog(&mut self, ctx: &egui::Context) {
        crate::features::plugins::render_dialog(ctx, &self.shared_state);
    }

    /// Render an open plugin page (replaces main content area)
    /// Returns true if a page is being rendered (caller should skip normal content)
    fn render_plugin_page(&mut self, ctx: &egui::Context) -> bool {
        crate::features::plugins::render_page(ctx, &self.shared_state)
    }
}
