//! Dialog handler for ArclainApp

use super::ArclainApp;
use crate::core::operations;
use crate::features::password_management;
use crate::shared::dialogs;
use eframe::egui;

pub fn render_dialogs(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Password Dialog
    let shared_state = app.shared_state.clone();
    match password_management::handle_password_dialogs(ctx, &shared_state) {
        password_management::PasswordFeatureAction::PasswordUnlocked { path, password } => {
            let mut archive_info = operations::archive::ArchiveInfo::default();
            let mut view_state = app.shared_state.signals().browser_view_state.get();
            let mut pass_dialog = app.shared_state.signals().password_dialog.get();
            let mut status_bar = app.shared_state.signals().status_bar.get();

            if operations::archive::try_open_with_password(
                &app.shared_state.app_state,
                &path,
                &password,
                &mut pass_dialog,
                &mut app._pending_archive_path,
                &mut status_bar,
                &mut view_state.view_entries,
                &mut archive_info,
            ) {
                app.shared_state
                    .signals()
                    .browser_view_state
                    .set(view_state);
                pass_dialog.show = false;
                // app.shared_state.signals().password_dialog.set(pass_dialog); // Updated below
                app._pending_archive_path = None;
            } else {
                pass_dialog.error = "Invalid password".to_string();
            }
            app.shared_state.signals().password_dialog.set(pass_dialog);
            app.shared_state.signals().status_bar.set(status_bar);
        }
        password_management::PasswordFeatureAction::None => {}
    }

    // Render Password Rules Dialog
    if let Some(result) = password_management::dialogs::zip_pass_rules::render_password_rules_dialog(
        ctx,
        &app.shared_state.theme,
        &mut app.settings_feature.password_rules_dialog,
    ) {
        match result {
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Cancel => {
                app.settings_feature.password_rules_dialog.show = false;
            }
            password_management::dialogs::zip_pass_rules::PasswordRulesResult::Save { rules } => {
                app.settings_feature.handle_action(
                    crate::features::settings::settings_content::SettingsAction::SavePasswordRules { rules },
                    &app.shared_state,
                );
                app.settings_feature.password_rules_dialog.show = false;
            }
        }
    }

    // Render Extraction Progress Dialog
    let mut ext_dialog = app.shared_state.signals().extraction_dialog.get();
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut ext_dialog,
    ) {
        match result {
            dialogs::progress::ExtractionDialogResult::Cancelled => {
                // Set signal-based cancellation for native backends
                app.shared_state
                    .signals()
                    .extraction_cancel
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                app.shared_state.signals().extraction_progress.set(None);
                // Also cancel CLI extraction if any
                app.archive_operations.cancel_extraction();
                ext_dialog.show = false;
            }
            dialogs::progress::ExtractionDialogResult::Minimized => {
                app.archive_operations.state_mut().extraction_minimized = true;
                ext_dialog.show = false;
            }
            dialogs::progress::ExtractionDialogResult::Paused => {
                app.archive_operations.pause_extraction();
            }
            dialogs::progress::ExtractionDialogResult::Resumed => {
                app.archive_operations.resume_extraction();
            }
            dialogs::progress::ExtractionDialogResult::None => {}
        }
    }
    app.shared_state.signals().extraction_dialog.set(ext_dialog);

    // Render Conversion Progress Dialog
    let mut conv_dialog = app.shared_state.signals().conversion_dialog.get();
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut conv_dialog,
    ) {
        match result {
            dialogs::progress::ExtractionDialogResult::Cancelled => {
                app.shared_state
                    .signals()
                    .extraction_cancel
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                // Note: Conversion cancellation logic needs to be implemented in ArchiveOperations if different
                // For now assuming it uses similar mechanism or child process kill
            }
            _ => {}
        }
    }
    app.shared_state
        .signals()
        .conversion_dialog
        .set(conv_dialog);

    // Render Drag Progress Dialog
    let mut drag_dialog = app.shared_state.signals().drag_dialog.get();
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut drag_dialog,
    ) {
        if let dialogs::progress::ExtractionDialogResult::Cancelled = result {
            drag_dialog.show = false;
        }
    }
    app.shared_state.signals().drag_dialog.set(drag_dialog);

    // Render File Edit Dialog
    let mut edit_dialog = app.shared_state.signals().file_edit_dialog.get();
    if let Some(result) = crate::features::file_editing::file_edit_dialog::render_file_edit_dialog(
        ctx,
        &app.shared_state.theme,
        &mut edit_dialog,
    ) {
        match result {
            crate::features::file_editing::file_edit_dialog::FileEditResult::Save {
                new_name,
                content,
            } => {
                if let Some(archive) = app.shared_state.signals().archive_path.get() {
                    let state = app.shared_state.app_state.lock();
                    let mut status = app.shared_state.signals().status_bar.get();
                    match state.add_or_update_file_from_str(&archive, &new_name, &content) {
                        Ok(_) => {
                            status.message = "File saved".to_string();
                            app.shared_state.signals().status_bar.set(status);
                            // TODO: Refresh file list
                        }
                        Err(e) => {
                            let msg = format!("Failed to save file: {}", e);
                            crate::core::utils::log_failure("FileEdit", &msg);
                            status.message = msg;
                            app.shared_state.signals().status_bar.set(status);
                        }
                    }
                }
                edit_dialog.show = false;
            }
            crate::features::file_editing::file_edit_dialog::FileEditResult::Cancel => {
                edit_dialog.show = false;
            }
        }
    }
    app.shared_state.signals().file_edit_dialog.set(edit_dialog);
}

pub fn render_overlays(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render toast notifications (always on top)
    app.shared_state.toaster.lock().show(ctx);

    // Render plugin dialog if open
    crate::features::plugins::render_dialog(ctx, &app.shared_state);

    // Render log viewer modal if open
    if app.show_log_viewer {
        let logs = if let Some(manager) = &app.shared_state.services.plugin_manager {
            manager.lock().get_network_log()
        } else {
            Vec::new()
        };
        dialogs::log_viewer::render(
            ctx,
            &app.shared_state.theme,
            &logs,
            &mut app.show_log_viewer,
        );
    }
}
