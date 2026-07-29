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

/// Open an extracted file with the system default handler. Returns
/// whether the OS spawn itself succeeded -- `crate::core::operation_bridge`'s
/// materialize-completion handler needs this to decide whether the
/// resulting lease is actually backing a launched application worth
/// renewing, or should just be released immediately since nothing is
/// going to read it (see [`open_extracted_file_via_signals`]).
fn open_extracted_file(file_path: &std::path::Path, status_message: &mut String) -> bool {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = std::process::Command::new("explorer")
            .arg(file_path)
            .spawn()
        {
            tracing::warn!("Failed to open file: {}", e);
            *status_message = format!("Failed to open file: {}", e);
            false
        } else {
            tracing::info!("Opened extracted file: {}", file_path.display());
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
            true
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
            false
        } else {
            *status_message = format!(
                "Opened: {}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            );
            true
        }
    }
}

/// Adapter for callers with no render-frame-local `&mut String` to thread
/// a status message through -- `crate::core::operation_bridge`'s
/// materialize-completion handler runs on the bridge's background worker,
/// not inside a frame, so it writes `shared_state.signals().status_bar`
/// directly instead. Reuses [`open_extracted_file`] verbatim rather than
/// duplicating its platform-conditional spawn logic. Returns the same
/// success/failure the underlying spawn reported.
pub(crate) fn open_extracted_file_via_signals(
    shared_state: &SharedState,
    file_path: &std::path::Path,
) -> bool {
    let mut message = String::new();
    let spawned = open_extracted_file(file_path, &mut message);
    shared_state.signals().status_bar.update(|s| {
        s.message = message.clone();
    });
    spawned
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
///
/// `tabs.json` (visual tab shape: id/pinned/active/order, entirely a
/// GUI concern) still supplies which tab each restored archive attaches
/// to, but `session.json` (the application-owned, frontend-neutral
/// `Vec<arclain_app::settings::SessionArchiveEntry>` `save_tabs_on_exit`
/// writes alongside it) is now the operational driver for *which paths
/// actually get reopened* — see [`resolve_session_restore_list`]'s own
/// doc comment.
pub fn restore_tabs_on_launch(shared_state: &SharedState) {
    let signals = shared_state.signals();
    let user_config = signals.user_config.get();
    if !user_config.restore_tabs_on_launch {
        return;
    }

    let config_dir = match arclain_app_fs::AppDirectories::init("arclain", None) {
        Ok(dirs) => dirs.config_dir,
        Err(e) => {
            tracing::warn!("[tabs] restore: could not resolve config dir: {}", e);
            return;
        }
    };
    let tabs_path = config_dir.join("tabs.json");

    if !tabs_path.exists() {
        return;
    }

    match crate::core::tabs::load_collection(&tabs_path) {
        Ok(restored) => {
            let session_path = config_dir.join("session.json");
            let tab_ids_to_load = resolve_session_restore_list(&session_path, &restored);

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

/// Resolves which archives to actually reopen on launch. The operational
/// driver is `session.json` (`arclain_app::settings::SessionArchiveEntry`,
/// this application's own frontend-neutral session file) -- not
/// `tabs.json`'s embedded per-tab `archive_path` directly. `tabs.json`
/// still supplies which UI tab (id/pinned/order) each restored path
/// attaches to; it no longer independently decides *whether* a path gets
/// reopened.
///
/// `session.json` and `tabs.json` are always written together, from the
/// same `col.tabs()` iteration, in the same relative order (see
/// `save_tabs_on_exit_to`): `session.json`'s entries are exactly
/// `tabs.json`'s own archive-bearing tabs, filtered to the ones that had
/// an archive open, in that same order. Matched here as parallel
/// sequences, position by position, confirming each pair's path agrees
/// before trusting it -- not merely checked for the same *set* of paths,
/// the way this function's predecessor (a passive cross-check that never
/// altered the restore) did. A position where the two disagree is
/// skipped rather than guessed at: reopening nothing is always safer
/// than reopening the wrong archive into the wrong tab.
///
/// Falls back to trusting every one of `restored`'s own embedded
/// `archive_path` values directly if `session.json` is missing or
/// unreadable -- an upgrade from a version that never wrote it, or a
/// `tabs.json` captured while `restore_tabs_on_launch` was disabled and
/// since re-enabled (see `save_tabs_on_exit_to`'s own doc comment for why
/// a disabled toggle actively removes any stale `session.json` it
/// finds). There is deliberately nothing durable to consult in that
/// case, so `tabs.json`'s already-loaded shape is the only source left.
fn resolve_session_restore_list(
    session_path: &std::path::Path,
    restored: &crate::core::tabs::TabsCollection,
) -> Vec<(crate::core::tabs::TabId, std::path::PathBuf)> {
    let archive_bearing_tabs: Vec<(crate::core::tabs::TabId, std::path::PathBuf)> = restored
        .tabs()
        .iter()
        .filter_map(|t| t.archive_path.get().map(|p| (t.id, p)))
        .collect();

    // `load_session_restore_list` itself reports a *missing* file as an
    // empty list (see its own doc comment) -- indistinguishable, from its
    // `Ok` return alone, from a real prior session that genuinely had
    // zero archives open. Those two cases need different handling here
    // (fall back to `tabs.json` vs. trust an authoritative "nothing was
    // open"), so this checks existence itself first, before ever calling
    // it.
    if !session_path.exists() {
        tracing::debug!(
            "[tabs] session.json does not exist; trusting tabs.json's own embedded archive paths"
        );
        return archive_bearing_tabs;
    }

    let session_entries = match arclain_app::settings::load_session_restore_list(session_path) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(
                "[tabs] session.json exists but failed to load ({error:?}); trusting tabs.json's \
                 own embedded archive paths"
            );
            return archive_bearing_tabs;
        }
    };

    if session_entries.len() != archive_bearing_tabs.len() {
        tracing::warn!(
            "[tabs] session.json ({} entries) disagrees with tabs.json ({} archive-bearing \
             tabs); restoring only the positions both agree on",
            session_entries.len(),
            archive_bearing_tabs.len()
        );
    }

    archive_bearing_tabs
        .into_iter()
        .zip(session_entries)
        .filter_map(|((tab_id, tabs_json_path), session_entry)| {
            if tabs_json_path == session_entry.source_path {
                Some((tab_id, tabs_json_path))
            } else {
                tracing::warn!(
                    "[tabs] tabs.json/session.json disagree at this position ({} vs {}); \
                     skipping this tab rather than reopening the wrong archive",
                    tabs_json_path.display(),
                    session_entry.source_path.display()
                );
                None
            }
        })
        .collect()
}

/// Save the current tab session to `tabs.json` in the config dir.
///
/// Called from `ArclainApp::on_exit`. Failures are logged as warnings
/// — a quit-time save failure should not block the shutdown.
///
/// Also writes `session.json` -- but only when `restore_tabs_on_launch`
/// is enabled: the same open-archive list, expressed as
/// `arclain_app::settings::SessionArchiveEntry` (source paths only, no
/// tab id/pinned/order — see that type's own doc comment). `tabs.json`
/// keeps the full visual arrangement (a GUI-only concern, always
/// saved); `session.json` is the one non-visual, application-owned
/// slice of it, and privacy demands it never records which archives a
/// user had open once they have told the application not to restore
/// them -- so a disabled toggle also removes any `session.json` left
/// over from an *earlier* session when the toggle was still enabled,
/// not just skips writing a new one.
pub fn save_tabs_on_exit(signals: &AppSignals) {
    let config_dir = match arclain_app_fs::AppDirectories::init("arclain", None) {
        Ok(dirs) => dirs.config_dir,
        Err(e) => {
            tracing::warn!("[tabs] on_exit: could not resolve config dir: {}", e);
            return;
        }
    };
    save_tabs_on_exit_to(&config_dir, signals);
}

/// The actual save logic, taking `config_dir` directly rather than
/// resolving it via `AppDirectories::init` itself -- split out purely
/// so tests can point it at a temp directory instead of this process's
/// real OS config dir, the same way `check_session_restore_list_matches`
/// already takes `config_dir` as a parameter rather than resolving it.
fn save_tabs_on_exit_to(config_dir: &std::path::Path, signals: &AppSignals) {
    let col = signals.tabs.get();
    let tabs_path = config_dir.join("tabs.json");
    if let Err(e) = crate::core::tabs::save_collection(&col, &tabs_path) {
        tracing::warn!("[tabs] failed to save {}: {}", tabs_path.display(), e);
    } else {
        tracing::info!("[tabs] session saved to {}", tabs_path.display());
    }

    let session_path = config_dir.join("session.json");
    if !signals.user_config.get().restore_tabs_on_launch {
        if let Err(error) = arclain_app::settings::clear_session_restore_list(&session_path) {
            tracing::warn!(
                "[tabs] failed to remove {}: {error:?}",
                session_path.display()
            );
        }
        return;
    }

    let session_entries: Vec<arclain_app::settings::SessionArchiveEntry> = col
        .tabs()
        .iter()
        .filter_map(|tab| {
            tab.archive_path
                .get()
                .map(|source_path| arclain_app::settings::SessionArchiveEntry { source_path })
        })
        .collect();
    if let Err(error) =
        arclain_app::settings::save_session_restore_list(&session_path, &session_entries)
    {
        tracing::warn!(
            "[tabs] failed to save {}: {error:?}",
            session_path.display()
        );
    }
}

/// Explicit, real shutdown of the application facade -- reclaims every
/// outstanding materialization lease directory (`ArclainApp::shutdown`'s
/// own `clear_all()`) and aborts the materialization cleanup task. Called
/// from `crate::core::arclain_app::ArclainApp::on_exit`, after
/// [`save_tabs_on_exit`]; a no-op if `shared_state.facade` is `None` (test
/// fixtures that skip a full bootstrap -- see `SharedState::facade`'s own
/// doc comment).
///
/// Before this was wired in anywhere, `on_exit` only saved tabs -- meaning
/// `ArclainApp::shutdown` never ran in the shipped application at all, no
/// matter how many times a user quit and relaunched: every doc comment
/// describing what `shutdown()` does was describing dead code as far as
/// the real binary was concerned.
///
/// `on_exit` is a synchronous eframe callback with no ambient async
/// runtime to `.await` on, so this drives the async `shutdown()` future to
/// completion on a freshly spawned, plain OS thread with its own
/// temporary single-threaded runtime -- matching `arclain_app`'s own
/// documented "await from any foreign runtime" contract rather than
/// assuming `shared_state.services.tokio_runtime`'s own internal state is
/// still in a known-good condition this late in the exit sequence. A real
/// OS thread (not just a new runtime *value* on the calling thread) also
/// guarantees this can never panic with "cannot start a runtime from
/// within a runtime", even if some future caller of this function ever
/// ran it from a context this crate does not control.
pub fn shutdown_facade_on_exit(shared_state: &SharedState) {
    let Some(app) = shared_state.facade.clone() else {
        return;
    };
    let outcome = std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => Some(runtime.block_on(app.shutdown())),
            Err(error) => {
                tracing::error!(
                    "[on_exit] failed to build a temporary runtime for shutdown: {error}"
                );
                None
            }
        }
    })
    .join();
    match outcome {
        Ok(Some(Ok(()))) => {}
        Ok(Some(Err(error))) => {
            tracing::error!("[on_exit] ArclainApp::shutdown reported an error: {error:?}");
        }
        Ok(None) => {} // already logged above
        Err(_) => {
            tracing::error!("[on_exit] the shutdown thread panicked");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tabs::TabsCollection;
    use arclain_app::settings::{save_session_restore_list, SessionArchiveEntry};
    use std::path::PathBuf;
    use tracing_test::traced_test;

    fn signals_with_restore(restore_tabs_on_launch: bool) -> AppSignals {
        let signals = AppSignals::new();
        signals.user_config.set(arclain_core::UserConfig {
            restore_tabs_on_launch,
            ..Default::default()
        });
        signals
    }

    /// The common (and by far most frequent) case: `session.json` and
    /// `tabs.json` were written together by the same `save_tabs_on_exit_to`
    /// call, so every position agrees -- every archive-bearing tab must be
    /// resolved for reopening, none skipped.
    #[test]
    fn resolve_session_restore_list_returns_every_tab_when_session_and_tabs_agree() {
        let dir = tempfile::tempdir().unwrap();
        let a = PathBuf::from("/tmp/a.zip");
        let b = PathBuf::from("/tmp/b.zip");
        save_session_restore_list(
            &dir.path().join("session.json"),
            &[
                SessionArchiveEntry {
                    source_path: a.clone(),
                },
                SessionArchiveEntry {
                    source_path: b.clone(),
                },
            ],
        )
        .expect("write session.json");

        let mut col = TabsCollection::new();
        let tab_a = col.open(Some(a.clone()));
        let tab_b = col.open(Some(b.clone()));

        let resolved = resolve_session_restore_list(&dir.path().join("session.json"), &col);

        assert_eq!(resolved, vec![(tab_a, a), (tab_b, b)]);
    }

    /// A genuine mismatch at one position (not merely a reorder) must be
    /// skipped rather than trusted -- reopening nothing for that tab is
    /// safer than reopening whatever `tabs.json` alone claims once the
    /// two files disagree on what belongs there.
    #[traced_test]
    #[test]
    fn resolve_session_restore_list_skips_a_disagreeing_position_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let agreeing = PathBuf::from("/tmp/agreeing.zip");
        save_session_restore_list(
            &dir.path().join("session.json"),
            &[
                SessionArchiveEntry {
                    source_path: PathBuf::from("/tmp/only-in-session.zip"),
                },
                SessionArchiveEntry {
                    source_path: agreeing.clone(),
                },
            ],
        )
        .expect("write session.json");

        let mut col = TabsCollection::new();
        let mismatched_tab = col.open(Some(PathBuf::from("/tmp/only-in-tabs.zip")));
        let agreeing_tab = col.open(Some(agreeing.clone()));

        let resolved = resolve_session_restore_list(&dir.path().join("session.json"), &col);

        assert_eq!(
            resolved,
            vec![(agreeing_tab, agreeing)],
            "the disagreeing position must be dropped; the agreeing one must still resolve"
        );
        let _ = mismatched_tab;
        assert!(logs_contain("disagree"));
    }

    /// A missing `session.json` (an upgrade from a version that never
    /// wrote it, or a re-enabled `restore_tabs_on_launch` toggle with no
    /// intervening save) falls back to trusting `tabs.json`'s own
    /// embedded archive paths directly -- there is nothing durable to
    /// consult, not a genuine disagreement to skip over.
    #[test]
    fn resolve_session_restore_list_falls_back_to_tabs_json_when_session_json_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let a = PathBuf::from("/tmp/a.zip");
        let mut col = TabsCollection::new();
        let tab_a = col.open(Some(a.clone()));

        let resolved = resolve_session_restore_list(&dir.path().join("session.json"), &col);

        assert_eq!(resolved, vec![(tab_a, a)]);
    }

    /// An `session.json` that *exists* but genuinely records zero open
    /// archives is authoritative, not a fallback trigger -- unlike a
    /// missing file (see the test above), this is a real prior session
    /// that had nothing open, and must not fall back to trusting
    /// `tabs.json` instead.
    #[test]
    fn resolve_session_restore_list_trusts_a_genuinely_empty_session_json() {
        let dir = tempfile::tempdir().unwrap();
        save_session_restore_list(&dir.path().join("session.json"), &[])
            .expect("write an empty session.json");

        let mut col = TabsCollection::new();
        col.open(Some(PathBuf::from("/tmp/a.zip")));

        let resolved = resolve_session_restore_list(&dir.path().join("session.json"), &col);

        assert_eq!(
            resolved,
            Vec::new(),
            "an existing, genuinely empty session.json must not fall back to tabs.json"
        );
    }

    /// The "fold 3" privacy fix: disabling `restore_tabs_on_launch`
    /// must remove any `session.json` left over from an earlier
    /// session (not just skip writing a new one) -- `tabs.json` (the
    /// GUI-only visual arrangement) is unaffected.
    #[test]
    fn save_tabs_on_exit_to_clears_a_stale_session_json_when_restore_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("session.json");
        save_session_restore_list(
            &session_path,
            &[SessionArchiveEntry {
                source_path: PathBuf::from("/tmp/stale.zip"),
            }],
        )
        .expect("seed a stale session.json");
        assert!(session_path.exists());

        let signals = signals_with_restore(false);
        let mut col = TabsCollection::new();
        col.open(Some(PathBuf::from("/tmp/currently-open.zip")));
        signals.tabs.set(col);

        save_tabs_on_exit_to(dir.path(), &signals);

        assert!(
            dir.path().join("tabs.json").exists(),
            "tabs.json (the GUI-only visual arrangement) must always be saved"
        );
        assert!(
            !session_path.exists(),
            "session.json must be removed once restore_tabs_on_launch is disabled, not just \
             left unwritten-to"
        );
    }

    #[test]
    fn save_tabs_on_exit_to_writes_session_json_when_restore_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let signals = signals_with_restore(true);
        let mut col = TabsCollection::new();
        col.open(Some(PathBuf::from("/tmp/currently-open.zip")));
        signals.tabs.set(col);

        save_tabs_on_exit_to(dir.path(), &signals);

        let entries =
            arclain_app::settings::load_session_restore_list(&dir.path().join("session.json"))
                .expect("load session.json");
        assert_eq!(
            entries,
            vec![SessionArchiveEntry {
                source_path: PathBuf::from("/tmp/currently-open.zip"),
            }]
        );
    }
}
