//! Main application coordinator
//!
//! The ArclainApp struct serves as the primary coordination point for the entire UI,
//! managing global state and delegating rendering to feature modules.

use crate::core::navigation::PageNavigator;
use crate::features::{organization, plugins, settings};
use crate::shared::components;

use eframe::egui;
use std::path::PathBuf;

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

    // UI Components
    pub(crate) top_tab_bar_state: components::top_tab_bar::TopTabBarState,

    // State & Flags
    pub(crate) _pending_archive_path: Option<PathBuf>,
    pub(crate) _last_window_title: Option<String>,
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
            settings_feature: settings::SettingsFeature::new(&shared_state),
            plugins_feature: plugins::PluginsFeature::new(&shared_state),
            organization_feature: organization::OrganizationFeature::new(&shared_state),
            top_tab_bar_state: components::top_tab_bar::TopTabBarState::new("archive"),
            _pending_archive_path: None,
            _last_window_title: None,
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
