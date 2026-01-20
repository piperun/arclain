//! Handler for file drop events
//!
//! Processes files dropped onto the application window.

use crate::core::arclain_app::ArclainApp;
use crate::core::{file_drop, operations};
use eframe::egui;

/// Handle file drop events from the OS
pub fn handle_drop_events(app: &mut ArclainApp, ctx: &egui::Context) {
    if let file_drop::DropAction::OpenArchive(path) = file_drop::process_dropped_files(ctx) {
        let mut archive_info = operations::archive::ArchiveInfo::default();
        let browser_state = app.archive_browser.state_mut();
        operations::archive::open_archive_by_path(
            &app.shared_state.app_state,
            &path,
            &mut browser_state.current_path,
            &mut app.password_feature.password_dialog,
            &mut app.status_info,
            &mut browser_state.entries,
            &mut archive_info,
        );
        // Switch to main page if not already there
        app.page_navigator.navigate_to_main();
    }
}
