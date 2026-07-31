//! Main application coordinator
//!
//! The ArclainApp struct serves as the primary coordination point for the entire UI,
//! managing global state and delegating rendering to feature modules.

use crate::core::navigation::PageNavigator;
use crate::features::{organization, plugins, settings};
use crate::shared::components;

use eframe::egui;

mod content_handler;
mod dialog_handler;
mod drop_handler;
mod toolbar_handler;
mod update;

/// Main application struct coordinating all features
pub struct ArclainApp {
    // Core shared state (contains app_state, theme, toaster, plugin_dialog_state)
    pub(crate) shared_state: crate::shared::SharedState,

    // Navigation
    pub(crate) page_navigator: PageNavigator,

    // UI State
    pub(crate) header_state: components::HeaderState,

    // Feature modules
    pub(crate) archive_browser: crate::features::archive_browser::ArchiveBrowser,
    pub(crate) archive_operations: crate::features::archive_operations::ArchiveOperations,
    pub(crate) settings_feature: settings::SettingsFeature,
    pub(crate) plugins_feature: plugins::PluginsFeature,
    pub(crate) organization_feature: organization::OrganizationFeature,
    pub(crate) hotkeys_feature: crate::features::hotkeys::HotkeysFeature,
    pub(crate) password_management_feature:
        crate::features::password_management::PasswordManagementFeature,

    // Hotkey management
    pub(crate) hotkey_manager: crate::features::hotkeys::HotkeyManager,

    // UI Components
    pub(crate) top_tab_bar_state: components::top_tab_bar::TopTabBarState,
    pub(crate) logs_page_state: crate::shared::components::logs_page::LogsPageState,
    pub(crate) process_state: crate::features::process::ProcessPageState,

    // State & Flags
    pub(crate) _last_window_title: Option<String>,
    pub(crate) _signals_bound: bool,
    pub(crate) _theme_applied: bool,
    pub(crate) _last_dark_mode: bool,
}

impl ArclainApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let log_session = crate::shared::components::logs_page::LogSession::capture_default();
        Self::new_with_log_session(cc, log_session)
    }

    pub fn new_with_log_session(
        cc: &eframe::CreationContext<'_>,
        log_session: crate::shared::components::logs_page::LogSession,
    ) -> Self {
        // Initialize shared state (includes AppState, Services, Theme)
        let shared_state = crate::shared::SharedState::new(cc);
        let archive_browser = crate::features::archive_browser::ArchiveBrowser::new(&shared_state);
        let archive_operations =
            crate::features::archive_operations::ArchiveOperations::new(&shared_state);
        let settings_feature = settings::SettingsFeature::new(&shared_state);
        let plugins_feature = plugins::PluginsFeature::new(&shared_state);
        let organization_feature = organization::OrganizationFeature::new(&shared_state);
        let hotkeys_feature = crate::features::hotkeys::HotkeysFeature::new(&shared_state);
        let password_management_feature =
            crate::features::password_management::PasswordManagementFeature::new(&shared_state);
        let last_dark_mode = shared_state.theme.dark_mode;

        Self {
            // Move the initialized state into the app after every feature
            // has taken the clones it needs.
            shared_state,
            page_navigator: PageNavigator::new(),
            header_state: components::HeaderState::default(),
            archive_browser,
            archive_operations,
            settings_feature,
            plugins_feature,
            organization_feature,
            hotkeys_feature,
            password_management_feature,
            hotkey_manager: crate::features::hotkeys::HotkeyManager::new(),
            top_tab_bar_state: components::top_tab_bar::TopTabBarState::new("archive"),
            logs_page_state: crate::shared::components::logs_page::LogsPageState::with_session(
                log_session,
            ),
            process_state: crate::features::process::ProcessPageState::new(),
            _last_window_title: None,
            _signals_bound: false,
            _theme_applied: false,
            _last_dark_mode: last_dark_mode,
        }
    }
}

impl eframe::App for ArclainApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::core::arclain_app::update::update_app(self, ctx, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Tab state first, then facade teardown: nothing about shutdown
        // affects what gets saved, but "capture state, then tear down" is
        // the natural ordering for an exit path, and it means a failure
        // in the (best-effort) shutdown step below can never prevent the
        // tab save from having already happened. See
        // `shutdown_facade_on_exit`'s own doc comment for why this call
        // exists at all and how it drives the async `shutdown()` future
        // to completion from this synchronous callback.
        crate::core::app_lifecycle::save_tabs_on_exit(&self.shared_state);
        crate::core::app_lifecycle::shutdown_facade_on_exit(&self.shared_state);
    }
}
