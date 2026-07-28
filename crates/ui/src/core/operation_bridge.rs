//! The egui operation bridge: a background worker that subscribes to
//! `ArclainApp`'s operation-event stream and forwards state to egui via
//! the existing per-tab signals, so the render thread never touches
//! `arclain_app` directly and never blocks on it.
//!
//! One worker per app, spawned once from `SharedState::new` onto the
//! shared Tokio runtime (the very runtime `ArclainApp`'s own operations
//! run on -- see `arclain_app::runtime`'s doc comment on why `Services::
//! tokio_runtime` and the facade's internal runtime are the same
//! instance). [`OperationOrigins`] tracks which tab a given
//! [`arclain_app::ids::OperationId`] belongs to, populated by whichever
//! call site starts the operation (`crate::core::operations::archive::
//! start_archive_open`, `crate::features::archive_operations::application::
//! extraction::start_extraction`); the worker reads it back for every
//! event and updates that tab's signals.
//!
//! Both operation kinds this task wires up (`OpenArchive`, `Extract`)
//! share a password-challenge dialog (the existing per-tab
//! `password_dialog` signal) rather than each owning a separate prompt --
//! see [`TabState::pending_challenge`]'s own doc comment for how the
//! render side knows which operation/challenge id a submitted password
//! answers.
//!
//! `Challenge::ConfirmOverwrite` has no interactive prompt wired up yet:
//! every egui-initiated extraction requests `CollisionPolicy::Overwrite`,
//! preserving the pre-facade UI's unconditional-overwrite behavior, so
//! this challenge is never raised by anything egui itself starts. The
//! facade fully supports it (see `arclain_app`'s own extraction-operation
//! tests) for a frontend that does ask for `CollisionPolicy::Ask`; this
//! worker still answers it (declining, so an operation can never hang
//! forever waiting on a prompt nobody will ever show) and logs a warning,
//! rather than silently ignoring it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use arclain_app::challenge::{Challenge, ChallengeResponse};
use arclain_app::event::{OperationEvent, OperationKind, OperationResult, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::ArclainApp;

use crate::core::tabs::TabId;
use crate::shared::dialogs::{ArchiveErrorDialogState, ArchiveErrorKind};
use crate::shared::SharedState;

/// Registry of in-flight operations' originating tab, shared between
/// whichever call site starts an operation ([`Self::register`]) and the
/// bridge worker (which resolves it for every event). A plain
/// `Mutex<HashMap<...>>`, mirroring `arclain_app::operations::
/// ChallengeWaiters`'s own shape for the equivalent "one slot per
/// in-flight operation" need.
#[derive(Clone, Default)]
pub struct OperationOrigins {
    origins: Arc<Mutex<HashMap<OperationId, TabId>>>,
}

impl OperationOrigins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `operation_id` as belonging to `tab_id`. Called
    /// immediately after `start_open_archive`/`start_extract` returns
    /// its id -- nothing could reference the id before that point, so
    /// there is no race against the bridge worker observing an event for
    /// it first.
    pub fn register(&self, operation_id: OperationId, tab_id: TabId) {
        self.origins.lock().unwrap().insert(operation_id, tab_id);
    }

    fn resolve(&self, operation_id: OperationId) -> Option<TabId> {
        self.origins.lock().unwrap().get(&operation_id).copied()
    }

    /// Drops the tracked origin once an operation reaches a terminal
    /// state -- otherwise this map grows for the lifetime of the
    /// application.
    fn forget(&self, operation_id: OperationId) {
        self.origins.lock().unwrap().remove(&operation_id);
    }
}

fn is_terminal(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Completed { .. }
            | OperationState::Cancelled
            | OperationState::Failed { .. }
    )
}

/// Best-effort classification of a facade error into the existing
/// archive-error dialog's coarser kind. `Generic` (showing the raw
/// diagnostic text) is always a safe fallback -- the dialog was designed
/// around a raw-string classifier before the facade existed; this task
/// does not attempt to grow it a new variant per `ApplicationErrorKind`,
/// only maps onto the ones that already have a dedicated dialog branch.
fn archive_error_kind(kind: arclain_app::error::ApplicationErrorKind) -> ArchiveErrorKind {
    use arclain_app::error::ApplicationErrorKind;
    match kind {
        ApplicationErrorKind::PermissionDenied => ArchiveErrorKind::PermissionDenied,
        ApplicationErrorKind::NotFound => ArchiveErrorKind::FileNotFound,
        _ => ArchiveErrorKind::Generic,
    }
}

/// Re-lists `path` directly through the backend selector to populate this
/// tab's flat `entries`/`archive_extras`/`opened_archive` signals -- the
/// data the archive browser UI reads today. A deliberate duplicate of
/// the facade's own internal listing: `arclain_app::ArchiveSession`
/// already holds an indexed copy of the same data behind
/// `list_entries`/`archive_snapshot`, but those are paginated,
/// hierarchical queries (`ArchiveEntryDto`), not the flat
/// `Vec<ArchiveEntry>` `TabState::entries` and the rest of the archive
/// browser were built around -- migrating the browser onto the paginated
/// facade model is a separate, much larger undertaking this task does
/// not attempt (see this task's report).
///
/// Resolves its own password rather than trusting `tab.current_password`
/// to already be right: a password the *user* typed in response to a
/// `Challenge::Password` does land there first (see
/// `handle_password_challenge`), but an *auto-detected* one (a matching
/// `PassRule`, resolved silently inside the facade's own session with no
/// challenge ever raised) does not -- there is nothing to read it back
/// from without reaching into the facade's private `ArchiveSession`
/// internals, which this crate must not do. Instead this mirrors
/// `archive_ops::attempt_initial`'s exact two-branch characterization
/// (see its own doc comment) using the identical `(pass_rules,
/// archive_name, last_entries)` inputs the facade resolved its own
/// password from, so it deterministically re-derives the same guess.
fn relist_for_browser_signals(
    shared: &SharedState,
    tab: &crate::core::tabs::TabState,
    path: &Path,
) -> anyhow::Result<()> {
    let (backend, pass_rules, last_entries) = {
        let state = shared.app_state.lock();
        (
            state.backend_selector.select(path)?,
            state.pass_rules.clone(),
            state.last_entries.clone(),
        )
    };
    let archive_name = path.to_str();
    let auto_password =
        || arclain_core::utilities::auto_password_for(&pass_rules, archive_name, &last_entries);

    let (info, resolved_password) = if let Some(password) = tab.current_password.get() {
        // Already known -- either a prior open of this same tab, or a
        // password the user just submitted for a live challenge.
        (backend.list(path, Some(&password))?, Some(password))
    } else {
        match backend.list(path, None) {
            Ok(info) if info.headers_encrypted => match auto_password() {
                Some(password) => match backend.list(path, Some(&password)) {
                    Ok(unlocked) => (unlocked, Some(password)),
                    Err(_) => (info, None),
                },
                None => (info, None),
            },
            Ok(info) => (info, None),
            Err(error) => match auto_password() {
                Some(password) => (backend.list(path, Some(&password))?, Some(password)),
                None => return Err(error),
            },
        }
    };

    if let Some(password) = &resolved_password {
        tab.current_password.set(Some(password.clone()));
    }

    tab.archive_path.set(Some(path.to_path_buf()));
    tab.archive_extras
        .set(crate::core::operations::archive::ArchiveExtras {
            archive_encrypted: info.encrypted,
            headers_encrypted: info.headers_encrypted,
            encryption_method: info.encryption_method.clone(),
        });
    tab.navigation
        .set(arclain_core::archive::NavigationState::new());
    {
        let mut view_state = tab.browser_view_state.get();
        if view_state.selection.clear() {
            tab.browser_view_state.set_if_changed(view_state);
        }
    }
    tab.selection_count.set_if_changed(0);
    tab.entries.set(Arc::new(info.entries));

    let archive = match resolved_password {
        Some(pw) => arclain_core::Archive::with_password(backend, path.to_path_buf(), pw),
        None => arclain_core::Archive::new(backend, path.to_path_buf()),
    };
    tab.opened_archive
        .set(Some(Arc::new(parking_lot::RwLock::new(archive))));

    crate::core::operations::navigation_view::refresh_view_entries_for_tab(
        shared.signals(),
        tab.id,
    );
    Ok(())
}

fn handle_open_archive_completed(
    shared: &SharedState,
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
    snapshot: arclain_app::archive::ArchiveSnapshot,
) {
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    tab.archive_session_id.set(Some(snapshot.session_id));
    let mut password_dialog = tab.password_dialog.get();
    password_dialog.show = false;
    password_dialog.error.clear();
    tab.password_dialog.set(password_dialog);
    tab.pending_challenge.set(None);

    if let Err(error) = relist_for_browser_signals(shared, &tab, &snapshot.source_path) {
        tracing::error!(
            "[operation_bridge] archive opened via the facade but the UI-side re-list failed: {error:#}"
        );
        shared.signals().status_bar.update(|status| {
            status.message = format!("Archive opened but failed to display: {error:#}");
        });
    } else {
        crate::core::operations::archive::finish_archive_load(shared, &tab);
        shared.signals().status_bar.update(|status| {
            status.message = "Archive loaded successfully".to_string();
        });

        // Auto-retry: if this open was triggered by a file-extraction
        // password failure (`process_extraction_progress`'s own prompt,
        // via `PasswordSubmittedForReopen`), re-fire `pending_open_file`
        // with the stashed path so the user's original click succeeds
        // without clicking again -- `tab.current_password` is already
        // set above.
        if let Some(retry_path) = tab.pending_open_after_unlock.get() {
            tab.pending_open_after_unlock.set(None);
            tab.pending_open_file.set(Some(retry_path));
        }
    }
    origins.forget(operation_id);
}

fn handle_open_archive_failed_or_cancelled(
    shared: &SharedState,
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
    error: Option<arclain_app::error::ApplicationError>,
) {
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    let mut password_dialog = tab.password_dialog.get();
    password_dialog.show = false;
    tab.password_dialog.set(password_dialog);
    tab.pending_challenge.set(None);

    if let Some(error) = error {
        let message = format!("Failed to load archive: {}", error.summary);
        tracing::error!("[operation_bridge] archive open failed: {error:?}");
        shared.signals().status_bar.update(|status| {
            status.message = message.clone();
        });
        shared
            .signals()
            .archive_error_dialog
            .set(ArchiveErrorDialogState {
                show: true,
                archive_path: error.path.clone(),
                kind: archive_error_kind(error.kind),
                raw_error: error.diagnostic.unwrap_or(error.summary),
                diagnostic: None,
            });
    } else {
        shared.signals().status_bar.update(|status| {
            status.message = "Archive open cancelled".to_string();
        });
    }
    origins.forget(operation_id);
}

fn handle_extract_progress(
    tab: &crate::core::tabs::TabState,
    percent: u64,
    message: Option<String>,
) {
    let mut dialog = tab.extraction_dialog().get();
    dialog.show = true;
    dialog.status = crate::shared::dialogs::ExtractionStatus::Running;
    dialog.percent = percent.min(100) as u8;
    if let Some(message) = message {
        if dialog.log_lines.len() > 500 {
            let overflow = dialog.log_lines.len() - 500;
            dialog.log_lines.drain(0..overflow);
        }
        dialog.log_lines.push(message.clone());
        dialog.file_action = message;
    }
    tab.extraction_dialog().set(dialog);
}

fn handle_extract_terminal(
    shared: &SharedState,
    tab_id: TabId,
    status: crate::shared::dialogs::ExtractionStatus,
    message: String,
) {
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    let mut dialog = tab.extraction_dialog().get();
    dialog.status = status;
    dialog.show = false;
    tab.extraction_dialog().set(dialog);
    tab.active_extraction_operation.set(None);
    shared.signals().status_bar.update(|s| {
        s.message = message;
    });
}

fn handle_password_challenge(
    shared: &SharedState,
    tab_id: TabId,
    operation_id: OperationId,
    challenge: Challenge,
) {
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    let Challenge::Password { attempt, .. } = &challenge else {
        return;
    };
    let mut dialog = tab.password_dialog.get();
    dialog.show = true;
    dialog.password.clear();
    dialog.error = if *attempt > 1 {
        "Incorrect password".to_string()
    } else {
        String::new()
    };
    tab.password_dialog.set(dialog);
    tab.pending_challenge
        .set(Some(super::tabs::PendingChallenge {
            operation_id,
            challenge,
        }));
}

fn handle_confirm_overwrite_challenge(
    app: &ArclainApp,
    runtime: &tokio::runtime::Runtime,
    operation_id: OperationId,
    challenge: Challenge,
) {
    let Challenge::ConfirmOverwrite { id, destination } = &challenge else {
        return;
    };
    tracing::warn!(
        "[operation_bridge] a ConfirmOverwrite challenge was raised for destination {} with no \
         interactive prompt wired up (every egui-initiated extraction requests \
         CollisionPolicy::Overwrite) -- auto-declining so the operation does not hang",
        destination.display()
    );
    let app = app.clone();
    let id = *id;
    runtime.spawn(async move {
        let _ = app
            .respond_to_challenge(
                operation_id,
                ChallengeResponse::ConfirmOverwrite {
                    id,
                    overwrite: false,
                },
            )
            .await;
    });
}

/// Handles one operation event, dispatching to the tab-specific handlers
/// above based on `event.kind`/`event.state`.
fn handle_event(
    shared: &SharedState,
    origins: &OperationOrigins,
    runtime: &tokio::runtime::Runtime,
    event: OperationEvent,
) {
    let Some(tab_id) = origins.resolve(event.operation_id) else {
        // Not one of ours (or already forgotten after a terminal event) --
        // every operation kind this bridge does not yet handle (convert,
        // organize, ...) is silently ignored the same way.
        return;
    };

    let event_is_terminal = is_terminal(&event.state);
    let event_operation_id = event.operation_id;
    match (&event.kind, event.state) {
        (_, OperationState::Challenge { challenge }) => match &challenge {
            Challenge::Password { .. } => {
                handle_password_challenge(shared, tab_id, event.operation_id, challenge)
            }
            Challenge::ConfirmOverwrite { .. } => {
                if let Some(app) = shared.facade.as_ref() {
                    handle_confirm_overwrite_challenge(app, runtime, event.operation_id, challenge)
                }
            }
            // No egui-initiated operation raises these today.
            Challenge::ConfirmDestructiveAction { .. }
            | Challenge::MissingExternalTool { .. }
            | Challenge::RetryPermission { .. } => {}
        },
        (
            OperationKind::Extract,
            OperationState::Progress {
                completed_units,
                message,
                ..
            },
        ) => {
            if let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() {
                handle_extract_progress(&tab, completed_units, message);
            }
        }
        (OperationKind::Extract, OperationState::Completed { .. }) => {
            handle_extract_terminal(
                shared,
                tab_id,
                crate::shared::dialogs::ExtractionStatus::Completed,
                "Extraction completed".to_string(),
            );
            origins.forget(event.operation_id);
        }
        (OperationKind::Extract, OperationState::Cancelled) => {
            handle_extract_terminal(
                shared,
                tab_id,
                crate::shared::dialogs::ExtractionStatus::Cancelled,
                "Extraction cancelled".to_string(),
            );
            origins.forget(event.operation_id);
        }
        (OperationKind::Extract, OperationState::Failed { error }) => {
            let message = format!("Extraction failed: {}", error.summary);
            handle_extract_terminal(
                shared,
                tab_id,
                crate::shared::dialogs::ExtractionStatus::Failed,
                message,
            );
            origins.forget(event.operation_id);
        }
        (
            OperationKind::OpenArchive,
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            },
        ) => handle_open_archive_completed(shared, origins, tab_id, event.operation_id, snapshot),
        (OperationKind::OpenArchive, OperationState::Cancelled) => {
            handle_open_archive_failed_or_cancelled(
                shared,
                origins,
                tab_id,
                event.operation_id,
                None,
            )
        }
        (OperationKind::OpenArchive, OperationState::Failed { error }) => {
            handle_open_archive_failed_or_cancelled(
                shared,
                origins,
                tab_id,
                event.operation_id,
                Some(error),
            )
        }
        _ => {
            if event_is_terminal {
                origins.forget(event_operation_id);
            }
        }
    }
}

/// Spawns the bridge worker onto `shared.services.tokio_runtime`. A no-op
/// if `shared.facade` is `None` (test fixtures that skip a full
/// `ArclainApp::bootstrap` -- see `SharedState::facade`'s own doc
/// comment). Reads `shared.operation_origins` directly -- the caller
/// (`SharedState::new`) constructs it before calling this, so every
/// clone of `shared` (including the one captured here) already shares
/// the same registry call sites register into.
pub fn spawn(shared: &SharedState) {
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let mut receiver = app.subscribe_operations();
    let shared = shared.clone();
    let runtime = shared.services.tokio_runtime.clone();
    runtime.clone().spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    handle_event(&shared, &shared.operation_origins, &runtime, event);
                    shared.signals().kick_repaint();
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
