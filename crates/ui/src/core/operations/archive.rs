use crate::core::tabs::{TabId, TabState};
use crate::shared::dialogs::MergeDialogState;
use crate::shared::SharedState;
use crc32fast::Hasher;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Starts opening `path` into `tab_id` through the application facade,
/// optionally with a password already in hand (the file-open retry flow
/// in `process_extraction_progress` supplies one when a single-file
/// extraction failed for want of a password -- everything else passes
/// `None` and lets the operation raise a `Challenge::Password` itself if
/// one turns out to be needed). Fire-and-forget: this function returns
/// as soon as the operation is dispatched, registering its id with
/// `shared`'s operation-bridge origins so that worker (see
/// `crate::core::operation_bridge`) can route progress/challenges/
/// completion back to `tab_id`. Every open in this crate funnels through
/// here now -- the file dialog, hotkey open, drop, tab restore/
/// duplicate/reopen-closed, and nested-archive-open call sites just
/// resolve a path and a target tab first.
pub fn start_archive_open(
    shared: &SharedState,
    tab_id: TabId,
    path: PathBuf,
    password: Option<String>,
) {
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[archive] start_archive_open: no application facade available");
        return;
    };
    if let Some(tab) = shared.signals().tabs.get().get(tab_id) {
        // Reset for a fresh open (mirrors the pre-facade `list_archive`'s
        // own `tab.current_password.set(None)`); reinstated immediately
        // when `password` is explicitly supplied (the "reopen with a
        // just-typed password" flow). Once the open completes, the
        // bridge re-stamps this from the session's own handle anyway --
        // covering the typed, rule-matched, and challenge-answered cases
        // alike -- so this early write only keeps the signal honest for
        // anything reading it while the open is still in flight.
        tab.current_password.set(password.clone());
    }
    let runtime = shared.services.tokio_runtime.clone();
    let shared = shared.clone();
    runtime.spawn(async move {
        match app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: path,
                password: password.map(arclain_app::challenge::SecretInput::new),
            })
            .await
        {
            Ok(operation_id) => {
                // Set before registering (not after): `register_operation`
                // immediately reconciles against the operation's current
                // snapshot, and a fast-failing open could already be
                // terminal by the time that reconciliation runs. Setting
                // this first means a terminal reconciliation's own
                // `tab.pending_open_operation.set(None)` is the value
                // that sticks, rather than this overwriting it back to
                // `Some` afterward for an operation that already finished.
                if let Some(tab) = shared.signals().tabs.get().get(tab_id) {
                    tab.pending_open_operation.set(Some(operation_id));
                }
                crate::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                    .await;
            }
            Err(error) => {
                tracing::error!("[archive] start_open_archive was rejected: {error:?}");
            }
        }
    });
}

/// Cancels the archive-open operation currently in flight for `tab`, if
/// any. Mirrors `crate::core::operations::extraction::cancel_extraction`
/// -- see the close-tab-confirm handler (`dialog_handler.rs`) for why an
/// in-flight operation must be cancelled before its owning tab goes
/// away: the facade has no way to notice a tab closing on its own, so
/// without this the open would keep running orphaned in the background
/// with nowhere left to route its completion once the tab is gone.
pub fn cancel_archive_open(shared: &SharedState, tab: &Arc<TabState>) {
    let Some(operation_id) = tab.pending_open_operation.get() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        let _ = app.cancel_operation(operation_id).await;
    });
}

/// Fire-and-forget `ArclainApp::close_archive` for `session_id`, if any.
///
/// Every call site that is about to overwrite or discard a tab's
/// `archive_session_id` -- closing the tab, replacing its active
/// archive with a different one -- must call this first with whatever
/// the id was beforehand. `close_archive` is the only way to release
/// the facade-side session `ArclainApp::start_open_archive` opened;
/// without a call here at every such site, the facade keeps the
/// session (and its indexed entry data) alive in memory for the rest of
/// the process's life. A no-op if `session_id` is `None` (nothing was
/// ever open) or the facade is unavailable (test fixtures).
pub fn close_archive_session(
    shared: &SharedState,
    session_id: Option<arclain_app::ids::ArchiveSessionId>,
) {
    let Some(session_id) = session_id else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        if let Err(error) = app.close_archive(session_id).await {
            tracing::warn!(
                "[archive] close_archive failed for a discarded session {session_id:?}: {error:?}"
            );
        }
    });
}

/// Handle opening an archive file via file dialog, into the active tab.
///
/// Multi-part detection happens here (before dispatching the open) since
/// it needs to redirect to the merge dialog instead -- the only call
/// site that ever did this pre-facade too.
pub fn open_archive_via_file_dialog(
    shared: &SharedState,
    merge_dialog: Option<&mut MergeDialogState>,
) {
    if let Some(file) = rfd::FileDialog::new()
        .add_filter("Archives", &["zip", "7z", "rar"])
        .pick_file()
    {
        info!("File selected: {}", file.display());

        if let Some(multipart) = arclain_app::archive::detect_multipart(&file) {
            if let Some(md) = merge_dialog {
                md.open(multipart);
                shared.signals().status_bar.update(|status| {
                    status.message =
                        "Multi-part archive detected. Use the dialog to merge.".to_string();
                });
                return;
            }
        }

        let active_id = shared.signals().tabs.get().active_id();
        start_archive_open(shared, active_id, file, None);
    }
}

/// Async archive loader for a specific tab -- the entry point for drop /
/// multi-tab loads, tab restore, tab duplicate, and reopen-closed-tab,
/// where the target tab is a specific (possibly non-active) `TabId`
/// rather than "whichever tab is active right now".
pub fn load_archive_into_tab(shared: &SharedState, tab_id: TabId, path: &std::path::Path) {
    start_archive_open(shared, tab_id, path.to_path_buf(), None);
}

/// Finishes an archive load once the facade's `start_open_archive`
/// operation has completed and `crate::core::operation_bridge` has
/// already seated the tab's session-backed listing/inventory: kicks off
/// the facade's encrypted-CRC backfill (unless the CRC policy is
/// `on_access`), refreshes the tab's rows when it landed anything, and
/// proactively shows the password dialog when the policy is
/// `prompt_on_open` and the backfill reports encrypted content with no
/// password anywhere in reach (neither the one the session was opened
/// with nor a stored rule matching the archive's name or its own entry
/// paths -- `ArclainApp::backfill_encrypted_crcs` owns that ladder now,
/// where the pre-facade code re-derived it from `AppState`'s own
/// rule/last-entries mirrors).
///
/// The policy split stays exactly where it was: this function is the
/// policy, the facade method is the mechanism. What moved is the
/// computation and its write -- the CRCs land in the session's own index
/// (visible to every consumer of the session's rows, not a private flat
/// list) and come back to this tab as a higher-revision refresh.
///
/// Runs as a spawned task rather than inline on the bridge's event
/// loop: the computation reads and hashes every targeted entry's
/// decrypted content, which for a large encrypted archive takes
/// arbitrarily long, and the pre-facade version blocked every other
/// tab's operation events for that whole duration. The visible ordering
/// change is deliberate and an improvement -- rows appear immediately
/// and their CRCs (or the password prompt) follow when ready.
pub fn finish_archive_load(shared: &SharedState, tab: &crate::core::tabs::TabState) {
    let policy = shared.app_state.lock().encrypted_crc_policy.clone();
    if policy == "on_access" {
        return;
    }
    let Some(session_id) = tab.archive_session_id.get() else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let Some(tab) = shared.signals().tabs.get().get(tab.id).cloned() else {
        return;
    };

    let shared = shared.clone();
    let runtime = shared.services.tokio_runtime.clone();
    runtime.clone().spawn(async move {
        let report = match app.backfill_encrypted_crcs(session_id).await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!("[archive] encrypted-CRC backfill failed: {error:?}");
                return;
            }
        };

        if report.computed > 0 {
            tracing::info!(
                "[archive] Computed CRC-32 for {} encrypted entries (policy={policy}).",
                report.computed
            );
            // The tab may have moved on to a different archive while the
            // computation ran; the refresh's own session guards drop a
            // stale answer, but skip the work outright when the binding
            // already changed.
            if tab.archive_session_id.get() != Some(session_id) {
                return;
            }
            if let Err(error) = crate::core::operation_bridge::refresh_entries_after_mutation(
                &shared, &tab, session_id,
            )
            .await
            {
                tracing::warn!("[archive] rows did not refresh after the CRC backfill: {error:?}");
            }
        }

        if policy == "prompt_on_open"
            && !report.password_available
            && report.any_encrypted
            && tab.archive_session_id.get() == Some(session_id)
        {
            let mut dialog = tab.password_dialog.get();
            dialog.show = true;
            dialog.password.clear();
            dialog.error.clear();
            tab.password_dialog.set(dialog);
            shared.signals().status_bar.update(|status| {
                status.message = "Password required to access encrypted content".to_string();
            });
        }
    });
}

/// Archive information state — output shape of the per-tab
/// `Computed<ArchiveInfo>` (see `TabState::archive_info`).
#[derive(Default, Clone)]
pub struct ArchiveInfo {
    pub archive_format: String,
    pub total_size: u64,
    pub compressed_size: u64,
    pub file_count: usize,
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
    pub total_crc32: Option<String>,
}

/// Non-derivable inputs to `ArchiveInfo` — the archive-level encryption
/// facts the backend's open-time listing surfaced, which no entry row
/// carries. Written by `crate::core::operation_bridge`'s open-completion
/// handler from `ArchiveSnapshot`'s own trio and read by the
/// `Computed<ArchiveInfo>` derivation on `TabState`.
#[derive(Default, Clone)]
pub struct ArchiveExtras {
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
}

/// Derive an [`ArchiveInfo`] from the tab's whole-archive inventory
/// rows. Pure function — the Computed closure on `TabState` is this with
/// the signals' `.get()` results plugged in.
///
/// Directory rows are excluded from every aggregate: unlike the flat
/// backend rows this used to sum (where a directory row carried zeros),
/// an inventory's directory rows carry the session's *recursive
/// aggregates*, and summing those alongside the files they aggregate
/// would count every byte once per ancestor. `file_count` counts actual
/// files for the same reason — the pre-facade number was the backend's
/// raw row count, which drifted with whether a backend happened to list
/// directories explicitly.
pub fn derive_archive_info(
    entries: &[arclain_app::archive::ArchiveEntryDto],
    archive_path: Option<&std::path::Path>,
    extras: &ArchiveExtras,
) -> ArchiveInfo {
    use arclain_app::archive::EntryKind;

    let archive_format = archive_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(|s| s.to_uppercase())
        .unwrap_or_default();

    let files = || {
        entries
            .iter()
            .filter(|entry| entry.kind != EntryKind::Directory)
    };
    let total_size = files().map(|entry| entry.uncompressed_size).sum();
    let compressed_size = files()
        .map(|entry| entry.compressed_size.unwrap_or(0))
        .sum();
    let file_count = files().count();

    // Aggregate per-entry CRC-32 into a single archive checksum.
    // Sorted path:crc pairs keep the result order-independent across
    // backends that list entries in different orders. Skipped for
    // empty / fully-encrypted archives (no entry CRCs available yet).
    // Directory rows are excluded exactly as before -- their crc32 is
    // itself an aggregate.
    let mut pairs: Vec<(String, String)> = files()
        .filter_map(|entry| {
            entry
                .crc32
                .as_ref()
                .map(|crc| (entry.path.as_str().to_string(), crc.to_uppercase()))
        })
        .collect();
    let total_crc32 = if pairs.is_empty() {
        None
    } else {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Hasher::new();
        for (p, c) in pairs {
            hasher.update(p.as_bytes());
            hasher.update(b":");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }
        Some(format!("{:08X}", hasher.finalize()))
    };

    ArchiveInfo {
        archive_format,
        total_size,
        compressed_size,
        file_count,
        archive_encrypted: extras.archive_encrypted,
        headers_encrypted: extras.headers_encrypted,
        encryption_method: extras.encryption_method.clone(),
        total_crc32,
    }
}

/// Detect whether a backend error message indicates a password
/// failure. Patterns cover the three backends arclain shells out to:
///
/// - 7-Zip CLI / native:  `Wrong password`, `code Some(2)`
/// - UnRAR CLI:           `Incorrect password`, `code Some(11)`
/// - UnRAR native:        `Password for encrypted archive not specified`
/// - Various:             `Cannot/Can not open encrypted`, `Enter password`,
///                        `code Some(255)`
///
/// Pub(crate) so the extraction-progress handler in
/// `app_lifecycle::process_extraction_progress` can reuse it for the
/// "show password dialog on extract failure" path. Keep this list
/// growing as new backends surface new patterns.
pub(crate) fn is_password_error(err_msg: &str) -> bool {
    err_msg.contains("Wrong password")
        || err_msg.contains("Incorrect password")
        || err_msg.contains("Password for encrypted archive not specified")
        || err_msg.contains("Cannot open encrypted")
        || err_msg.contains("Can not open encrypted")
        || err_msg.contains("Enter password")
        || err_msg.contains("code Some(2)")
        || err_msg.contains("code Some(11)")
        || err_msg.contains("code Some(255)")
}
