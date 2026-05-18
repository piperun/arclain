//! Handler for file drop events
//!
//! Processes files dropped onto the application window.

use crate::core::arclain_app::ArclainApp;
use crate::core::{file_drop, operations};
use arclain_core::archive::MultiPartArchive;
use eframe::egui;

/// Handle file drop events from the OS
pub fn handle_drop_events(app: &mut ArclainApp, ctx: &egui::Context) {
    if let file_drop::DropAction::OpenArchive(path) = file_drop::process_dropped_files(ctx) {
        // Check if this is a multi-part archive
        if let Some(multipart) = MultiPartArchive::detect(&path) {
            // Show merge dialog instead of opening directly
            let mut merge_dialog = app.shared_state.signals().merge_dialog.get();
            merge_dialog.open(multipart);
            app.shared_state.signals().merge_dialog.set(merge_dialog);

            let mut status_bar = app.shared_state.signals().status_bar.get();
            status_bar.message =
                "Multi-part archive detected. Use the dialog to merge.".to_string();
            app.shared_state.signals().status_bar.set(status_bar);

            // Still switch to main page
            app.page_navigator.navigate_to_main();
            return;
        }

        let mut archive_info = operations::archive::ArchiveInfo::default();
        let t = app.shared_state.signals().tabs.get().active().clone();
        let mut view_state = t.browser_view_state.get();
        // nav removed
        let mut pass_dialog = app.shared_state.signals().password_dialog.get();
        let mut status_bar = app.shared_state.signals().status_bar.get();

        operations::archive::open_archive_by_path(
            &app.shared_state.app_state,
            &path,
            // current_path removed
            &mut pass_dialog,
            &mut status_bar,
            &mut view_state.view_entries,
            &mut archive_info,
        );
        // navigation set removed
        t.browser_view_state.set(view_state);
        app.shared_state.signals().password_dialog.set(pass_dialog);
        app.shared_state.signals().status_bar.set(status_bar);
        // Switch to main page if not already there
        app.page_navigator.navigate_to_main();
    }
}
