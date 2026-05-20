//! Handler for file drop events
//!
//! Processes files dropped onto the application window.
//!
//! NOTE: As of Phase 2b, the drop overlay in `dialog_handler::render_overlays`
//! handles all archive drops (single-tab and multi-tab alike). This function
//! now only handles multi-part archive detection, which the overlay does not
//! handle. Plain single-archive drops are skipped here to avoid double-opens.

use crate::core::arclain_app::ArclainApp;
use crate::core::file_drop;
use arclain_core::archive::MultiPartArchive;
use eframe::egui;

/// Handle file drop events from the OS
pub fn handle_drop_events(app: &mut ArclainApp, ctx: &egui::Context) {
    if let file_drop::DropAction::OpenArchive(path) = file_drop::process_dropped_files(ctx) {
        // Multi-part archive detection: show the merge dialog instead of
        // opening directly. The drop overlay does not handle multi-part
        // archives, so this branch must stay here.
        if let Some(multipart) = MultiPartArchive::detect(&path) {
            // merge_dialog is per-tab now (post 2026-05-20 audit B2 follow-up)
            let active_tab = app.shared_state.signals().tabs.get().active().clone();
            let mut merge_dialog = active_tab.merge_dialog.get();
            merge_dialog.open(multipart);
            active_tab.merge_dialog.set(merge_dialog);

            let mut status_bar = app.shared_state.signals().status_bar.get();
            status_bar.message =
                "Multi-part archive detected. Use the dialog to merge.".to_string();
            app.shared_state.signals().status_bar.set(status_bar);

            app.page_navigator.navigate_to_main();
            return;
        }

        // Single archive: handled by the drop overlay in render_overlays.
        // Nothing to do here — navigate to main so the loaded archive is visible.
        app.page_navigator.navigate_to_main();
    }
}
