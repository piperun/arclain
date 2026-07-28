//! Application lifecycle management
//!
//! Handles per-frame lifecycle tasks like signal binding, theme application,
//! metadata updates, and extraction progress handling.

use crate::core::signals::AppSignals;
use crate::features::organization;
use crate::shared::{dialogs, SharedState};
use eframe::egui;
use std::sync::atomic::Ordering;

/// Process plugin refresh requests
pub fn process_refresh_requests(shared_state: &SharedState, ctx: &egui::Context) {
    if shared_state.refresh_requests.swap(false, Ordering::AcqRel) {
        tracing::debug!("Processing plugin layout refresh request");
        shared_state.plugin_ui_jobs.invalidate_all_layouts();
        let dialog_signal = shared_state.signals().plugin_dialog_state.clone();
        let mut dialog_state = dialog_signal.get();
        dialog_state.invalidate_dialog_layout();
        dialog_state.invalidate_page_layout();
        dialog_signal.set(dialog_state);
        ctx.request_repaint();
    }
}

/// Bind signals to egui context for automatic repaint
pub fn bind_signals_once(shared: &SharedState, ctx: &egui::Context, bound: &mut bool) {
    if !*bound {
        let state = shared.app_state.lock();
        state.signals.bind_to_context(ctx);
        drop(state);
        let signal_context = arclain_signals::SignalContext::new(ctx.clone());
        signal_context.bind_named(
            shared.plugin_ui_jobs.completion_signal(),
            "plugin_ui_completion_epoch",
        );
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

/// Check for and process metadata updates from plugin signals.
///
/// Walks EVERY tab, not just the active one — the dispatch worker
/// writes each event's metadata into its originating tab's signal
/// (per `PluginEvent::OnArchiveOpen.metadata_signal`), and that tab
/// might not be the active one by the time the worker finishes.
/// Pre-walk, drag-dropping 5 archives left tabs 1–4 with
/// `Some(metadata)` sitting unconsumed in their signal — their
/// `game_metadata` stayed `None` and the status-bar chip / Process
/// page output naming only populated when the user manually
/// switched to each tab. The per-frame walk consumes each tab
/// independently so every open's metadata reaches its tab on the
/// next frame.
pub fn process_metadata_signal(
    shared_state: &SharedState,
    organization_feature: &mut organization::OrganizationFeature,
) {
    let col = shared_state.signals().tabs.get();
    let active_id = col.active_id();

    // Last-consumed wins for the status-bar message. If several
    // tabs consume in one frame (typical for a batch drag-drop) the
    // user sees the latest title surface on the left. Imperfect UX
    // for batches but better than no feedback; richer aggregation
    // (e.g. "Found 5 archives") can come later.
    let mut latest_summary: Option<String> = None;

    for tab in col.tabs() {
        let Some(json_val) = tab.metadata.get() else {
            continue;
        };
        tab.metadata.set(None); // Consume

        let json_str = json_val.to_string();
        let meta = match arclain_core::features::organization::GameMetadata::from_json(&json_str) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Failed to parse metadata from signal: {}", e);
                continue;
            }
        };

        tracing::info!(
            "Received metadata signal update for tab {:?}: {} (ID: {})",
            tab.id,
            meta.title,
            meta.product_id
        );

        latest_summary = Some(if !meta.title.is_empty() {
            format!("Found: {} [{}]", meta.title, meta.product_id)
        } else {
            format!("Found metadata for {}", meta.product_id)
        });

        // Populate THIS tab's `game_metadata` — not the active
        // tab's. The chip in the status bar reads
        // `active_tab.game_metadata`, so the user only sees a chip
        // for the tab they're looking at, but every tab now has
        // the right value when they switch to it.
        tab.game_metadata.set(Some(meta.clone()));

        // The Organizer is a global UI singleton tied to the active
        // tab — only push metadata into it when the consumed tab IS
        // the active one; otherwise we'd overwrite the organizer
        // with whichever tab consumed last (rarely the right one).
        if tab.id == active_id {
            if let Some(page) = &mut organization_feature.organizer_page {
                page.panel.session.metadata = Some(meta);
                page.panel.update_preview();
                tracing::info!("Updated Organization Panel with new metadata");
            }
        }
    }

    if let Some(summary) = latest_summary {
        shared_state.signals().status_bar.update(|s| {
            s.message = summary;
        });
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
                    // the active tab — the existing unlock flow sets
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

                        *status_message = "Archive contents are password-protected".to_string();
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
                    started_at: None,
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
///
/// `pub(crate)`, not private: `crate::core::operation_bridge` also calls
/// this once a `file_opener::open_file_from_archive` materialization
/// completes and the extracted content turns out to itself be an archive.
pub(crate) fn open_nested_archive_in_tab(
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
            crate::core::operations::archive::close_archive_session(
                shared_state,
                col.active().archive_session_id.get(),
            );
            col.replace_active(archive_path.to_path_buf());
            col.active_id()
        } else {
            col.open(Some(archive_path.to_path_buf()))
        }
    };
    signals.tabs.set(col);

    crate::core::operations::archive::load_archive_into_tab(
        shared_state,
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

/// Adapter for callers with no render-frame-local `&mut String` to thread
/// a status message through -- `crate::core::operation_bridge`'s
/// materialize-completion handler runs on the bridge's background worker,
/// not inside a frame, so it writes `shared_state.signals().status_bar`
/// directly instead. Reuses [`open_extracted_file`] verbatim rather than
/// duplicating its platform-conditional spawn logic.
pub(crate) fn open_extracted_file_via_signals(
    shared_state: &SharedState,
    file_path: &std::path::Path,
) {
    let mut message = String::new();
    open_extracted_file(file_path, &mut message);
    shared_state.signals().status_bar.update(|s| {
        s.message = message.clone();
    });
}

/// Restore the previous tab session from `tabs.json` in the config dir.
///
/// Called once during `SharedState::new()`, after `shared` itself is
/// fully built (so the archive-open operations this spawns can register
/// with the operation bridge same as any other open). Reads
/// `restore_tabs_on_launch` from the already-loaded `user_config` signal.
/// If enabled and the file exists, replaces the default single-tab
/// collection and spawns background loads for every tab that had an
/// archive open. Missing files surface through the existing status-bar
/// error path — no special toast in v1.
pub fn restore_tabs_on_launch(shared_state: &SharedState) {
    let signals = shared_state.signals();
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
                    shared_state,
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
        if let Some(path) = shared_state
            .signals()
            .tabs
            .get()
            .active()
            .archive_path
            .get()
        {
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
