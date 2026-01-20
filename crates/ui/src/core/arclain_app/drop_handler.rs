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
        let mut view_state = app.shared_state.signals().browser_view_state.get();
        operations::archive::open_archive_by_path(
            &app.shared_state.app_state,
            &path,
            &mut view_state.current_path,
            &mut app.password_feature.password_dialog,
            &mut app.status_info,
            &mut view_state.view_entries,
            &mut archive_info,
        );
        app.shared_state
            .signals()
            .browser_view_state
            .set(view_state);
        // Switch to main page if not already there
        app.page_navigator.navigate_to_main();
    }
}
