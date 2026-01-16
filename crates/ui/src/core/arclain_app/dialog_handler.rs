//! Dialog handler for ArclainApp

use super::ArclainApp;
use crate::core::operations;
use crate::features::password_management;
use crate::shared::dialogs;
use eframe::egui;

pub fn render_dialogs(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render Password Dialog
    let shared_state = app.shared_state.clone();
    match password_management::handle_password_dialogs(
        &mut app.password_feature,
        ctx,
        &shared_state,
    ) {
        password_management::PasswordFeatureAction::PasswordUnlocked { path, password } => {
            let mut archive_info = operations::archive::ArchiveInfo::default();
            let browser_state = app.archive_browser.state_mut();
            if operations::archive::try_open_with_password(
                &app.shared_state.app_state,
                &path,
                &password,
                &mut app.password_feature.password_dialog,
                &mut app._pending_archive_path,
                &mut app.status_info,
                &mut browser_state.entries,
                &mut archive_info,
            ) {
                app.password_feature.password_dialog.show = false;
                app._pending_archive_path = None;
            } else {
                app.password_feature.password_dialog.error = "Invalid password".to_string();
            }
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
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut app.archive_operations.state_mut().extraction_dialog,
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
                app.archive_operations.state_mut().extraction_dialog.show = false;
            }
            dialogs::progress::ExtractionDialogResult::Minimized => {
                app.archive_operations.state_mut().extraction_minimized = true;
                app.archive_operations.state_mut().extraction_dialog.show = false;
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

    // Render Drag Progress Dialog
    if let Some(result) = dialogs::progress::render_extraction_progress_dialog(
        ctx,
        &app.shared_state.theme,
        &mut app.archive_operations.state_mut().drag_dialog,
    ) {
        if let dialogs::progress::ExtractionDialogResult::Cancelled = result {
            app.archive_operations.state_mut().drag_dialog.show = false;
        }
    }

    // Render File Edit Dialog
    if let Some(result) = crate::features::file_editing::file_edit_dialog::render_file_edit_dialog(
        ctx,
        &app.shared_state.theme,
        &mut app.edit_dialog,
    ) {
        match result {
            crate::features::file_editing::file_edit_dialog::FileEditResult::Save {
                new_name,
                content,
            } => {
                if let Some(archive) = app.shared_state.signals().archive_path.get() {
                    let state = app.shared_state.app_state.lock();
                    match state.add_or_update_file_from_str(&archive, &new_name, &content) {
                        Ok(_) => {
                            app.status_info.message = "File saved".to_string();
                            // TODO: Refresh file list
                        }
                        Err(e) => {
                            let msg = format!("Failed to save file: {}", e);
                            crate::core::utils::log_failure("FileEdit", &msg);
                            app.status_info.message = msg;
                        }
                    }
                }
                app.edit_dialog.show = false;
            }
            crate::features::file_editing::file_edit_dialog::FileEditResult::Cancel => {
                app.edit_dialog.show = false;
            }
        }
    }
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
