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
            app.shared_state
                .signals()
                .password_dialog
                .set_if_changed(pass_dialog);
            app.shared_state
                .signals()
                .status_bar
                .set_if_changed(status_bar);
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
    app.shared_state
        .signals()
        .extraction_dialog
        .set_if_changed(ext_dialog);

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
        .set_if_changed(conv_dialog);

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
    app.shared_state
        .signals()
        .drag_dialog
        .set_if_changed(drag_dialog);

    // Render File Edit Dialog
    let mut edit_dialog = app.shared_state.signals().file_edit_dialog.get();
    if let Some(result) = crate::features::file_editing::render_file_edit_dialog(
        ctx,
        &app.shared_state.theme,
        &mut edit_dialog,
    ) {
        match result {
            crate::features::file_editing::FileEditResult::Save { new_name, content } => {
                if let Some(archive) = app.shared_state.signals().archive_path.get() {
                    let mut status = app.shared_state.signals().status_bar.get();

                    // Save the file to archive
                    let save_result = {
                        let state = app.shared_state.app_state.lock();
                        state.add_or_update_file_from_str(&archive, &new_name, &content)
                    };

                    match save_result {
                        Ok(_) => {
                            status.message = "File saved".to_string();

                            // Re-list the archive to update entries signal
                            let mut state = app.shared_state.app_state.lock();
                            if let Ok(_) = state.list_archive(&archive) {
                                // Refresh the browser view from updated entries
                                crate::core::operations::navigation_view::refresh_view_entries(
                                    &state.signals,
                                );
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to save file: {}", e);
                            crate::core::utils::log_failure("FileEdit", &msg);
                            status.message = msg;
                        }
                    }
                    app.shared_state.signals().status_bar.set_if_changed(status);
                }
                edit_dialog.show = false;
            }
            crate::features::file_editing::FileEditResult::Cancel => {
                edit_dialog.show = false;
            }
        }
    }
    app.shared_state
        .signals()
        .file_edit_dialog
        .set_if_changed(edit_dialog);

    // Render Merge Dialog
    let mut merge_dialog = app.shared_state.signals().merge_dialog.get();
    match dialogs::render_merge_dialog(ctx, &app.shared_state.theme, &mut merge_dialog) {
        dialogs::MergeDialogResult::StartMerge => {
            // Clone needed data before triggering merge
            if let Some(ref multipart) = merge_dialog.multipart {
                let multipart_clone = multipart.clone();
                let output_format = merge_dialog.output_format;
                let compression_level = merge_dialog.compression_level;
                let delete_originals = merge_dialog.delete_originals;
                let password = if merge_dialog.password.is_empty() {
                    None
                } else {
                    Some(merge_dialog.password.clone())
                };

                // Get output path (same directory as first part)
                let output_path = multipart_clone.first_part.parent().map(|p| {
                    p.join(format!(
                        "{}.{}",
                        multipart_clone.base_name,
                        output_format.extension()
                    ))
                });

                // Trigger merge operation in background
                let backend_selector = {
                    let state = app.shared_state.app_state.lock();
                    state.backend_selector.clone()
                };

                let signals = app.shared_state.signals().clone();
                let mut status = app.shared_state.signals().status_bar.get();

                // Use tokio runtime from services
                let runtime = app.shared_state.services.tokio_runtime.clone();

                runtime.spawn(async move {
                    use arclain_core::services::{MergeOptions, MergeService};

                    let merge_service = MergeService::new(backend_selector);
                    let mut mp = multipart_clone;

                    let options = MergeOptions {
                        output_format,
                        output_path,
                        compression_level,
                        delete_originals,
                        password,
                    };

                    // Update status to show merge in progress
                    let mut extraction_dialog = signals.extraction_dialog.get();
                    extraction_dialog.show = true;
                    extraction_dialog.title = "Merging Archive".to_string();
                    extraction_dialog.file_action = format!("Merging {} parts...", mp.all_parts.len());
                    extraction_dialog.percent = 0;
                    extraction_dialog.can_pause = false;
                    extraction_dialog.can_minimize = false;
                    extraction_dialog.can_cancel = false;
                    signals.extraction_dialog.set(extraction_dialog);

                    match merge_service.merge(&mut mp, options, None, None) {
                        Ok(result_path) => {
                            let mut extraction_dialog = signals.extraction_dialog.get();
                            extraction_dialog.show = false;
                            signals.extraction_dialog.set(extraction_dialog);

                            let mut sb = signals.status_bar.get();
                            sb.message = format!(
                                "Merge complete: {}",
                                result_path.file_name().unwrap_or_default().to_string_lossy()
                            );
                            signals.status_bar.set(sb);
                        }
                        Err(e) => {
                            let mut extraction_dialog = signals.extraction_dialog.get();
                            extraction_dialog.show = false;
                            signals.extraction_dialog.set(extraction_dialog);

                            let mut sb = signals.status_bar.get();
                            sb.message = format!("Merge failed: {}", e);
                            signals.status_bar.set(sb);
                        }
                    }
                });

                status.message = "Starting merge...".to_string();
                app.shared_state.signals().status_bar.set(status);
            }
            merge_dialog.close();
        }
        dialogs::MergeDialogResult::Cancel => {
            // Dialog already closed in render function
        }
        dialogs::MergeDialogResult::None => {}
    }
    app.shared_state
        .signals()
        .merge_dialog
        .set_if_changed(merge_dialog);
}

pub fn render_overlays(app: &mut ArclainApp, ctx: &egui::Context) {
    // Render toast notifications (always on top)
    app.shared_state.toaster.lock().show(ctx);

    // Render plugin dialog if open
    crate::features::plugins::presentation::views::rendering::render_dialog(ctx, &app.shared_state);

    // Render lightbox if open
    let mut lightbox_state = app.shared_state.signals().lightbox_state.get();
    if lightbox_state.show {
        let result = dialogs::render_lightbox(
            ctx,
            &app.shared_state.theme,
            &mut lightbox_state,
            app.shared_state.services.content_cache.as_ref(),
        );
        match result {
            dialogs::LightboxResult::Closed => {
                // State already closed in render function
            }
            dialogs::LightboxResult::ImageChanged(_index) => {
                // Could notify plugin if needed
            }
            dialogs::LightboxResult::None => {}
        }
        app.shared_state
            .signals()
            .lightbox_state
            .set_if_changed(lightbox_state);
    }
}
