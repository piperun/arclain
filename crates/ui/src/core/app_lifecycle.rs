//! Application lifecycle management
//!
//! Handles per-frame lifecycle tasks like signal binding, theme application,
//! metadata updates, and extraction progress handling.

use crate::core::signals::AppSignals;
use crate::core::state::AppState;
use crate::features::organization;
use crate::shared::{dialogs, SharedState};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

/// Process plugin refresh requests
pub fn process_refresh_requests(shared_state: &SharedState, ctx: &egui::Context) {
    let mut requests = shared_state.refresh_requests.lock();
    if !requests.is_empty() {
        tracing::debug!(
            "Processing {} refresh requests: {:?}",
            requests.len(),
            requests
        );
        requests.clear();
        ctx.request_repaint();
    }
}

/// Bind signals to egui context for automatic repaint
pub fn bind_signals_once(app_state: &Arc<Mutex<AppState>>, ctx: &egui::Context, bound: &mut bool) {
    if !*bound {
        let state = app_state.lock();
        state.signals.bind_to_context(ctx);
        drop(state);
        *bound = true;
        tracing::info!("Signals bound to egui context");
    }
}

/// Apply theme to context and set widget theme colors
pub fn apply_theme(shared_state: &SharedState, ctx: &egui::Context) {
    shared_state.theme.apply_to_context(ctx);
    arclain_widgets::set_theme(ctx, shared_state.theme.colors.clone());
}

/// Check for hotkey input and return triggered actions
///
/// This should be called early in the update loop before rendering.
/// Returns a list of triggered actions to be dispatched by the caller.
pub fn process_hotkey_input(
    hotkey_manager: &crate::features::hotkeys::HotkeyManager,
    ctx: &egui::Context,
) -> Vec<crate::features::hotkeys::HotkeyAction> {
    hotkey_manager.check_input(ctx)
}

/// Check for and process metadata updates from plugin signals
pub fn process_metadata_signal(
    shared_state: &SharedState,
    organization_feature: &mut organization::OrganizationFeature,
) {
    let new_metadata = {
        let tab = shared_state.signals().tabs.get().active().clone();
        let val = tab.metadata.get();
        if val.is_some() {
            tab.metadata.set(None); // Consume
            val
        } else {
            None
        }
    };

    if let Some(json_val) = new_metadata {
        let json_str = json_val.to_string();
        match arclain_core::features::organization::GameMetadata::from_json(&json_str) {
            Ok(meta) => {
                tracing::info!(
                    "Received metadata signal update: {} (ID: {})",
                    meta.title,
                    meta.product_id
                );

                // Update per-tab game_metadata signal
                shared_state.signals().tabs.get().active().game_metadata.set(Some(meta.clone()));
                // Note: metadata already consumed (set to None) on line 63

                // Update active organizer panel
                if let Some(page) = &mut organization_feature.organizer_page {
                    page.panel.session.metadata = Some(meta);
                    page.panel.update_preview();
                    tracing::info!("Updated Organization Panel with new metadata");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to parse metadata from signal: {}", e);
            }
        }
    }
}

/// Handle extraction progress from native backends
pub fn process_extraction_progress(
    shared_state: &SharedState,
    extraction_dialog: &mut dialogs::ExtractionProgressDialog,
    status_message: &mut String,
    ctx: &egui::Context,
) {
    let progress_opt = shared_state.signals().extraction_progress.get();

    if let Some(progress) = progress_opt {
        if progress.complete {
            // Extraction finished - clear the signal
            shared_state.signals().extraction_progress.set(None);

            // Handle file opening
            if let Some(file_path) = progress.file_to_open {
                if file_path.exists() {
                    // If the extracted file is itself an archive, route
                    // through arclain's archive-open flow instead of the
                    // OS default handler. Lets us a) keep nested-archive
                    // browsing inside the app, and b) surface the
                    // password dialog when the inner archive is
                    // encrypted (the file_opener's extraction succeeded
                    // because the OUTER archive wasn't encrypted, but
                    // listing the inner one trips the password path in
                    // load_archive_into_tab's Err arm).
                    if arclain_core::features::organization::flatten::is_archive_extension(
                        &file_path,
                    ) {
                        open_nested_archive_in_tab(shared_state, &file_path);
                    } else {
                        open_extracted_file(&file_path, status_message);
                    }
                } else {
                    tracing::warn!("Extracted file not found: {}", file_path.display());
                    *status_message = "Extraction completed but file not found".to_string();
                }
            } else {
                *status_message = "Extraction completed".to_string();
            }

            // Handle errors
            if let Some(error) = &progress.error {
                tracing::error!("Extraction failed: {}", error);
                if error.contains("cancelled") {
                    *status_message = "Extraction cancelled".to_string();
                } else if crate::core::operations::archive::is_password_error(error) {
                    // The archive the user is extracting from has
                    // encrypted contents and no password has been
                    // provided yet. Surface the password dialog on
                    // the active tab — the existing unlock flow
                    // (dialog_handler → try_open_with_password) sets
                    // tab.current_password on success, and the unlock
                    // handler re-fires `pending_open_file` from
                    // `pending_open_after_unlock` so the same click
                    // succeeds automatically.
                    let active = shared_state.signals().tabs.get().active().clone();
                    if let Some(archive_path) = active.archive_path.get() {
                        let mut pwd = active.password_dialog.get();
                        pwd.show = true;
                        pwd.password.clear();
                        pwd.error = "Archive contents are password-protected".to_string();
                        pwd.target_path = Some(archive_path);
                        active.password_dialog.set(pwd);

                        // Stash the originally-requested file path so
                        // the unlock handler can auto-retry the open
                        // without making the user click again.
                        active
                            .pending_open_after_unlock
                            .set(progress.requested_file_path.clone());

                        *status_message =
                            "Archive contents are password-protected".to_string();
                    } else {
                        *status_message = format!("Extraction failed: {}", error);
                    }
                } else {
                    *status_message = format!("Extraction failed: {}", error);
                }
            }

            extraction_dialog.show = false;
        } else {
            // Update extraction dialog with current progress
            if !extraction_dialog.show {
                // First progress update - show the dialog
                *extraction_dialog = dialogs::ExtractionProgressDialog {
                    show: true,
                    title: "Extracting files".to_string(),
                    file_action: format!("Extracting {} files...", progress.total),
                    percent: progress.percent,
                    processed_text: format!("{}/{}", progress.current, progress.total),
                    elapsed_text: String::new(),
                    time_left_text: String::new(),
                    status: dialogs::ExtractionStatus::Running,
                    can_minimize: false,
                    can_pause: false,
                    can_cancel: true,
                    error: String::new(),
                    log_lines: vec![progress.current_file.clone()],
                    show_log: false,
                    dest_path: None,
                };
            } else {
                extraction_dialog.percent = progress.percent;
                extraction_dialog.processed_text =
                    format!("{}/{}", progress.current, progress.total);
                extraction_dialog.file_action = progress.current_file.clone();
            }

            ctx.request_repaint();
        }
    }
}

/// Open a freshly-extracted nested archive inside arclain (rather
/// than handing it off to the OS default handler).
///
/// Honors the user's `open_nested_in_new_tab` setting:
///   - `true` (default): open the nested archive in a new tab.
///   - `false`: replace the active tab's archive with the nested one.
///
/// Falls through `load_archive_into_tab`, so the encryption path
/// (password dialog show) and auto-bind ctx-repaint behave the same
/// as a top-level archive open.
fn open_nested_archive_in_tab(
    shared_state: &SharedState,
    archive_path: &std::path::Path,
) {
    let signals = shared_state.signals();
    let user_config = signals.user_config.get();
    let open_in_new_tab = user_config.open_nested_in_new_tab;
    drop(user_config);

    let mut col = signals.tabs.get();
    let target_tab_id = if open_in_new_tab {
        col.open(Some(archive_path.to_path_buf()))
    } else {
        // Replace the active tab when there's an archive in it, or
        // reuse the empty placeholder when there isn't.
        if col.active().archive_path.get().is_some() {
            col.replace_active(archive_path.to_path_buf());
            col.active_id()
        } else {
            col.open(Some(archive_path.to_path_buf()))
        }
    };
    signals.tabs.set(col);

    crate::core::operations::archive::load_archive_into_tab(
        shared_state.app_state.clone(),
        signals.clone(),
        target_tab_id,
        archive_path,
    );
}

/// Open an extracted file with the system default handler
fn open_extracted_file(file_path: &std::path::Path, status_message: &mut String) {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = std::process::Command::new("explorer")
            .arg(file_path)
            .spawn()
        {
            tracing::warn!("Failed to open file: {}", e);
            *status_message = format!("Failed to open file: {}", e);
        } else {
            tracing::info!("Opened extracted file: {}", file_path.display());
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Err(e) = std::process::Command::new("xdg-open")
            .arg(file_path)
            .spawn()
        {
            tracing::warn!("Failed to open file: {}", e);
            *status_message = format!("Failed to open file: {}", e);
        } else {
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
}

/// Restore the previous tab session from `tabs.json` in the config dir.
///
/// Called once during `SharedState::new()`. Reads `restore_tabs_on_launch`
/// from the already-loaded `user_config` signal. If enabled and the file
/// exists, replaces the default single-tab collection and spawns background
/// loads for every tab that had an archive open. Missing files surface
/// through the existing status-bar error path — no special toast in v1.
pub fn restore_tabs_on_launch(
    app_state: &Arc<Mutex<AppState>>,
    signals: &AppSignals,
) {
    let user_config = signals.user_config.get();
    if !user_config.restore_tabs_on_launch {
        return;
    }

    let tabs_path = match arclain_app_fs::AppDirectories::init("arclain", None) {
        Ok(dirs) => dirs.config_dir.join("tabs.json"),
        Err(e) => {
            tracing::warn!("[tabs] restore: could not resolve config dir: {}", e);
            return;
        }
    };

    if !tabs_path.exists() {
        return;
    }

    match crate::core::tabs::load_collection(&tabs_path) {
        Ok(restored) => {
            // Collect (tab_id, archive_path) pairs before moving the collection.
            let tab_ids_to_load: Vec<(crate::core::tabs::TabId, std::path::PathBuf)> = restored
                .tabs()
                .iter()
                .filter_map(|t| t.archive_path.get().map(|p| (t.id, p)))
                .collect();

            signals.tabs.set(restored);

            for (tab_id, path) in tab_ids_to_load {
                crate::core::operations::archive::load_archive_into_tab(
                    app_state.clone(),
                    signals.clone(),
                    tab_id,
                    &path,
                );
            }

            tracing::info!("[tabs] session restored from {}", tabs_path.display());
        }
        Err(e) => {
            tracing::warn!(
                "[tabs] failed to restore from {}: {}; starting with default tabs",
                tabs_path.display(),
                e
            );
        }
    }
}

/// Save the current tab session to `tabs.json` in the config dir.
///
/// Called from `ArclainApp::on_exit`. Failures are logged as warnings
/// — a quit-time save failure should not block the shutdown.
pub fn save_tabs_on_exit(signals: &AppSignals) {
    let tabs_path = match arclain_app_fs::AppDirectories::init("arclain", None) {
        Ok(dirs) => dirs.config_dir.join("tabs.json"),
        Err(e) => {
            tracing::warn!("[tabs] on_exit: could not resolve config dir: {}", e);
            return;
        }
    };

    let col = signals.tabs.get();
    if let Err(e) = crate::core::tabs::save_collection(&col, &tabs_path) {
        tracing::warn!("[tabs] failed to save {}: {}", tabs_path.display(), e);
    } else {
        tracing::info!("[tabs] session saved to {}", tabs_path.display());
    }
}

/// Update window title based on current state
pub fn update_window_title(
    shared_state: &SharedState,
    page_navigator: &crate::core::navigation::PageNavigator,
    last_title: &mut Option<String>,
    ctx: &egui::Context,
) {
    let title = {
        if let Some(path) = shared_state.signals().tabs.get().active().archive_path.get() {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Arclain".to_string())
        } else if let Some(settings_page) = page_navigator.current_settings_page() {
            format!("Settings - {}", settings_page.display_name())
        } else {
            "Arclain".to_string()
        }
    };

    let sanitized = crate::core::operations::window::sanitize_window_title(&title);
    if last_title.as_deref() != Some(&sanitized) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(sanitized.clone()));
        *last_title = Some(sanitized);
    }
}
