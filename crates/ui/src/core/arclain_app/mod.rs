//! Main application coordinator
//!
//! The ArclainApp struct serves as the primary coordination point for the entire UI,
//! managing global state and delegating rendering to feature modules.

use crate::core::navigation::PageNavigator;
use crate::features::{organization, plugins, settings};
use crate::shared::components;

use eframe::egui;
// use parking_lot::Mutex;
use std::path::PathBuf;
// use std::sync::Arc;

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

    // Feature states will be added here as we extract them
    // TODO: Add feature-specific state structs

    // UI State
    pub(crate) header_state: components::HeaderState,
    // Feature states
    pub(crate) archive_browser: crate::features::archive_browser::ArchiveBrowser,
    pub(crate) archive_operations: crate::features::archive_operations::ArchiveOperations,

    // Dialog states
    // pub(crate) password_feature: password_management::PasswordFeature,
    // pub(crate) edit_dialog: crate::features::file_editing::FileEditDialog,
    // password_rules_dialog: password_management::PasswordRulesDialog, // Moved to SettingsFeature

    // Settings state
    pub(crate) settings_feature: settings::SettingsFeature,
    // security_settings_state: settings::SecuritySettingsState,
    // archives_settings_state: settings::ArchivesSettingsState,
    // plugins_state: plugins::PluginsListState, // Moved to SettingsFeature
    pub(crate) plugins_feature: plugins::PluginsFeature,
    pub(crate) organization_feature: organization::OrganizationFeature,

    // Top tab bar state
    pub(crate) top_tab_bar_state: components::top_tab_bar::TopTabBarState,

    // Data
    // pub(crate) status_info: components::StatusBarInfo,
    pub(crate) _pending_archive_path: Option<PathBuf>,
    // _pending_open_file: Option<String>, // Moved to ArchiveOperations

    // Archive info
    // pub(crate) archive_info: operations::archive::ArchiveInfo, // Duplicated from AppState
    pub(crate) _last_window_title: Option<String>,

    // Extraction progress state - Moved to ArchiveOperations
    // extraction_dialog: dialogs::progress::ExtractionProgressDialog,
    // _extraction_rx: Option<Receiver<ProgressUpdate>>,
    // _extraction_child: Option<std::process::Child>,
    // extraction_minimized: bool,
    // _extraction_started: Option<Instant>,
    pub(crate) _password_rules_loaded: bool,
    pub(crate) _signals_bound: bool,
    pub(crate) _theme_applied: bool,
    pub(crate) _last_dark_mode: bool,
    pub(crate) show_log_viewer: bool,
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
            // password_feature: password_management::PasswordFeature::new(&shared_state),
            // edit_dialog: crate::features::file_editing::FileEditDialog::default(),
            // password_rules_dialog: password_management::PasswordRulesDialog::default(),
            settings_feature: settings::SettingsFeature::new(&shared_state),
            // security_settings_state: settings::SecuritySettingsState::default(),
            // archives_settings_state: settings::ArchivesSettingsState::default(),
            // plugins_state: plugins::PluginsListState::default(),
            plugins_feature: plugins::PluginsFeature::new(&shared_state),
            organization_feature: organization::OrganizationFeature::new(&shared_state),
            top_tab_bar_state: components::top_tab_bar::TopTabBarState::new("archive"),
            // status_info: components::StatusBarInfo::default(), // Removed
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
            _theme_applied: false,
            _last_dark_mode: shared_state.theme.dark_mode,
            show_log_viewer: false,
        }
    }
}

impl eframe::App for ArclainApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::core::arclain_app::update::update_app(self, ctx, frame);
    }
}
