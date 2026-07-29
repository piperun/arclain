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
//! extraction::start_extraction`, `crate::features::archive_operations::
//! application::file_opener::open_file_from_archive`); the worker reads it
//! back for every event and updates that tab's signals.
//! [`MaterializationActions`] carries the extra, materialize-specific "what
//! to do on completion" a bare tab id has no room for (see its own doc
//! comment); [`ExternalOpenLeases`] tracks which materialization leases an
//! external-open action is keeping alive by periodic renewal.
//!
//! Every operation kind this bridge wires up shares one password-challenge
//! dialog (the existing per-tab `password_dialog` signal) rather than each
//! owning a separate prompt -- see [`TabState::pending_challenge`]'s own
//! doc comment for how the render side knows which operation/challenge id
//! a submitted password answers.
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
//!
//! A second, independent worker subscribes to `ArclainApp`'s
//! *session*-event stream (`arclain_app::event::SessionEvent`) alongside
//! the operation one above -- see [`handle_session_event`]'s own doc
//! comment for why this is a second task rather than one merged loop.
//! Session-scoped changes (currently: a plugin's metadata/rename write
//! through `arclain_app::plugins::ArchiveContextBridge`) happen outside
//! any operation, so they have no `OperationId`/[`OperationOrigins`]
//! entry to resolve a tab through -- this worker instead re-fetches
//! `archive_snapshot` and matches by the session id every tab's own
//! `archive_session_id` signal already carries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arclain_app::challenge::{Challenge, ChallengeResponse};
use arclain_app::event::{
    OperationEvent, OperationKind, OperationResult, OperationState, SessionEvent,
};
use arclain_app::ids::{MaterializationLeaseId, OperationId};
use arclain_app::materialization::MaterializationLease;
use arclain_app::ArclainApp;

use crate::core::tabs::TabId;
use crate::shared::dialogs::{ArchiveErrorDialogState, ArchiveErrorKind};
use crate::shared::SharedState;

/// Upper bound on how many sessions' worth of metadata this bridge will
/// hold in `AppSignals::pending_session_metadata` while waiting for
/// their tab to be stamped with the matching `archive_session_id`. A
/// `SessionEvent::MetadataChanged` can arrive for a session no tab is
/// stamped with yet (a plugin's `OnArchiveOpen` handler can call back
/// with metadata before `handle_open_archive_completed` -- a fully
/// independent consumer of the same operation-completion event --
/// stamps the originating tab), and every such session is bounded here
/// exactly like `OperationOrigins`/`MaterializationActions` bound their
/// own per-operation maps: a made-up or already-superseded session id
/// would otherwise grow this map forever, since nothing ever drains an
/// entry whose tab never gets stamped. Reaching this cap is itself a
/// signal something is wrong, not normal use, so the response is to drop
/// the *new* entry and log a warning, not silently evict a legitimate
/// older one that might still be about to be claimed.
const MAX_PENDING_SESSION_METADATA: usize = 64;

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

    /// Registers `operation_id` as belonging to `tab_id`.
    ///
    /// Called immediately after `start_open_archive`/`start_extract`
    /// returns its id -- but that is *not* race-free: the spawned
    /// worker starts running (and can publish events, including a
    /// terminal one for a fast-failing operation) concurrently with the
    /// caller resuming from that `.await` and reaching this call. The
    /// bridge's receiver task can observe and drop such an event before
    /// this ever runs. Callers must use
    /// `crate::core::operation_bridge::register_operation` instead of
    /// calling this directly -- it registers and then immediately
    /// reconciles against the operation's current snapshot, so a state
    /// reached before registration is never silently lost. This method
    /// stays `pub` only for `register_operation` itself and tests.
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

    /// Every operation id currently tracked, snapshotted at call time.
    /// Used after the broadcast receiver reports `Lagged` to reconcile
    /// every in-flight operation directly against its current snapshot
    /// (see `reconcile_after_lag`) -- a dropped event can otherwise
    /// leave an origin (and its tab's dialog) stuck forever.
    fn tracked_ids(&self) -> Vec<OperationId> {
        self.origins.lock().unwrap().keys().copied().collect()
    }
}

/// What to do once a `Materialize` operation this bridge is tracking
/// completes. Keyed by `OperationId` in [`MaterializationActions`],
/// alongside (not instead of) [`OperationOrigins`]: `OperationOrigins`
/// answers "which tab", this answers "what specifically to do for this one
/// materialize call" -- per-call-site information `OperationOrigins`'s
/// simpler, operation-kind-agnostic shape has no room for.
#[derive(Clone, Debug)]
pub enum MaterializationAction {
    /// Launch the materialized content in the OS's default external
    /// application (or, if it is itself an archive, open it as a nested
    /// archive in this tab instead) -- `crate::features::archive_operations::
    /// application::file_opener`'s replacement for the pre-facade leaked
    /// `FileOpener`. `relative_target` is `Some` when the lease represents
    /// a whole directory (materializing a target file's containing folder
    /// so sibling files -- a game executable's co-located DLLs -- come
    /// along too) and names the specific file within it to actually open;
    /// `None` when the lease's own `local_path` already is that file.
    ExternalOpen { relative_target: Option<String> },
}

/// Registry of pending [`MaterializationAction`]s for in-flight `Materialize`
/// operations. A plain `Mutex<HashMap<...>>`, mirroring [`OperationOrigins`]'s
/// own shape for the equivalent "one slot per in-flight operation" need.
#[derive(Clone, Default)]
pub struct MaterializationActions {
    actions: Arc<Mutex<HashMap<OperationId, MaterializationAction>>>,
}

impl MaterializationActions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `action` for `operation_id`. Called immediately after
    /// `start_materialization` returns its id, mirroring
    /// `OperationOrigins::register`'s own no-race reasoning.
    pub fn register(&self, operation_id: OperationId, action: MaterializationAction) {
        self.actions.lock().unwrap().insert(operation_id, action);
    }

    /// Removes and returns the pending action for `operation_id`, if any --
    /// there is exactly one terminal event per operation (see
    /// `arclain_app::operations::registry`'s own guarantee), so taking by
    /// value here can never double-fire an action.
    fn take(&self, operation_id: OperationId) -> Option<MaterializationAction> {
        self.actions.lock().unwrap().remove(&operation_id)
    }
}

/// Materialization leases currently backing a launched external
/// application, kept alive by periodic renewal (`renew_due_external_open_leases`,
/// called once per frame from `crate::core::arclain_app::update`) for as
/// long as this application session runs.
///
/// There is no reliable, portable way to detect "the external, OS-
/// registered application has finished reading this file and exited" for
/// an arbitrary launched handler (unlike a nested nested-archive open,
/// which reads the file once up front and is done with it -- see
/// `handle_materialize_completed`, which releases that case's lease
/// immediately instead of tracking it here). Renewing indefinitely for the
/// life of the session, rather than guessing at a release point, is the
/// deliberate trade-off: `ArclainApp::shutdown`'s own cleanup reclaims
/// every directory still tracked here when the application actually
/// exits, and an individual lease still expires on its own (see
/// `arclain_app::materialization`'s default TTL) if renewal itself ever
/// stops (a crash, a bug) rather than leaking forever the way the
/// pre-facade `std::mem::forget` did.
#[derive(Clone, Default)]
pub struct ExternalOpenLeases {
    leases: Arc<Mutex<HashMap<MaterializationLeaseId, Instant>>>,
}

impl ExternalOpenLeases {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts tracking `lease_id`, timestamped as just-renewed (it was
    /// materialized moments ago, which already gave it a full, fresh TTL).
    pub fn track(&self, lease_id: MaterializationLeaseId) {
        self.leases.lock().unwrap().insert(lease_id, Instant::now());
    }

    /// Every tracked lease last renewed at least `min_age` ago -- due for
    /// another renewal call.
    fn due_for_renewal(&self, min_age: Duration) -> Vec<MaterializationLeaseId> {
        let now = Instant::now();
        self.leases
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, last)| now.duration_since(**last) >= min_age)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Records a successful renewal, resetting the tracked lease's clock.
    fn mark_renewed(&self, lease_id: MaterializationLeaseId) {
        if let Some(last) = self.leases.lock().unwrap().get_mut(&lease_id) {
            *last = Instant::now();
        }
    }

    /// Stops tracking a lease that failed to renew -- already released or
    /// expired out from under this tracker, so there is nothing left to
    /// renew.
    fn forget(&self, lease_id: MaterializationLeaseId) {
        self.leases.lock().unwrap().remove(&lease_id);
    }
}

/// How long a tracked external-open lease is allowed to sit since its last
/// renewal before the next per-frame check renews it again. Comfortably
/// under `arclain_app::materialization::DEFAULT_LEASE_TTL` (5 minutes) so a
/// slow frame or a busy runtime never risks letting a lease actually expire
/// while still tracked.
const EXTERNAL_OPEN_RENEWAL_INTERVAL: Duration = Duration::from_secs(60);

/// Renews every tracked external-open lease due for it. Called once per
/// frame from `crate::core::arclain_app::update`; cheap on every frame
/// where nothing is due (a single lock plus a linear scan over what is
/// expected to be a very small set -- one entry per currently-open
/// external application).
pub fn renew_due_external_open_leases(shared: &SharedState) {
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let due = shared
        .external_open_leases
        .due_for_renewal(EXTERNAL_OPEN_RENEWAL_INTERVAL);
    if due.is_empty() {
        return;
    }
    let leases = shared.external_open_leases.clone();
    // Marks every due id renewed *now*, optimistically, before the actual
    // (async) renewal call below even starts -- not after it completes.
    // This call runs once per frame; without this, a lease found "due"
    // here would still read as due on every subsequent frame until the
    // spawned task below actually finishes and calls `mark_renewed`
    // itself, so a slow frame cadence relative to the renewal round trip
    // could spawn a duplicate renewal for the same lease many times over
    // for what is logically one "it's due" event. If the real renewal
    // call fails, the `Err` arm below still calls `forget`, so an
    // optimistic mark here never masks a genuine failure -- it only
    // widens the window during which this same lease is treated as
    // "already handled" for this round.
    for &lease_id in &due {
        leases.mark_renewed(lease_id);
    }
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        for lease_id in due {
            match app.renew_materialization(lease_id).await {
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        "[operation_bridge] failed to renew external-open lease {lease_id:?}, \
                         no longer tracking it: {error:?}"
                    );
                    leases.forget(lease_id);
                }
            }
        }
    });
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

/// The pieces of `relist_for_browser_signals` that come from actual
/// backend I/O, plain data with no `TabState`/`SharedState` access --
/// see `resolve_archive_listing`'s own doc comment for why this is kept
/// separate.
struct ResolvedListing {
    info: arclain_core::archive::ArchiveInfo,
    resolved_password: Option<String>,
}

/// The blocking half of `relist_for_browser_signals`: lists `path`
/// through `backend`, trying `current_password` first, then falling
/// back to auto-detecting one from `pass_rules`/`last_entries` exactly
/// the way `archive_ops::attempt_initial` does (see its own doc
/// comment) using the identical inputs the facade resolved its own
/// password from, so it deterministically re-derives the same guess --
/// there is nothing to read the facade's own resolved password back
/// from without reaching into its private `ArchiveSession` internals,
/// which this crate must not do.
///
/// Pure data in, data out: no signal access, so this can run inside
/// `spawn_blocking` without a blocking-pool thread ever touching
/// `SharedState`/`TabState`. `backend.list()` is a real blocking
/// filesystem/subprocess call (7z-CLI or a native archive library), and
/// this is invoked from the bridge worker's own event loop (see
/// `spawn`'s doc comment) -- running it inline on that loop would block
/// every other tab's operation events (and the broadcast channel
/// itself) for as long as the listing takes.
fn resolve_archive_listing(
    backend: Arc<dyn arclain_core::ArchiveBackend>,
    pass_rules: Vec<arclain_core::utilities::PassRule>,
    last_entries: Vec<String>,
    path: PathBuf,
    current_password: Option<String>,
) -> anyhow::Result<ResolvedListing> {
    let archive_name = path.to_str();
    let auto_password =
        || arclain_core::utilities::auto_password_for(&pass_rules, archive_name, &last_entries);

    let (info, resolved_password) = if let Some(password) = current_password {
        // Already known -- either a prior open of this same tab, or a
        // password the user just submitted for a live challenge.
        (backend.list(&path, Some(&password))?, Some(password))
    } else {
        match backend.list(&path, None) {
            Ok(info) if info.headers_encrypted => match auto_password() {
                Some(password) => match backend.list(&path, Some(&password)) {
                    Ok(unlocked) => (unlocked, Some(password)),
                    Err(_) => (info, None),
                },
                None => (info, None),
            },
            Ok(info) => (info, None),
            Err(error) => match auto_password() {
                Some(password) => (backend.list(&path, Some(&password))?, Some(password)),
                None => return Err(error),
            },
        }
    };
    Ok(ResolvedListing {
        info,
        resolved_password,
    })
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
/// The actual backend listing (`resolve_archive_listing`) runs inside
/// `spawn_blocking` -- see that function's own doc comment for why
/// running it directly on the bridge's event loop would be a problem.
async fn relist_for_browser_signals(
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
    let current_password = tab.current_password.get();
    let path_owned = path.to_path_buf();
    // `backend` is `Arc<dyn ArchiveBackend>` -- cloned (a cheap refcount
    // bump) rather than moved outright, since `Archive::new`/
    // `with_password` below still need a handle to it after the
    // blocking task below consumes its own copy.
    let backend_for_blocking = backend.clone();
    let ResolvedListing {
        info,
        resolved_password,
    } = tokio::task::spawn_blocking(move || {
        resolve_archive_listing(
            backend_for_blocking,
            pass_rules,
            last_entries,
            path_owned,
            current_password,
        )
    })
    .await
    .map_err(|join_error| anyhow::anyhow!("archive listing task panicked: {join_error}"))??;

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

    // Refresh the auto-password matcher's "internal file paths" input
    // (see `resolve_archive_listing`'s own doc comment on
    // `auto_password_for`) with this archive's own entries. Left stale
    // (whatever was set at app startup, or by whichever archive was
    // opened first this session), a `PassRule` keyed on an entry
    // filename could only ever match the very first archive it was
    // ever checked against.
    {
        let mut state = shared.app_state.lock();
        state.last_entries = info
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
    }
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

/// Removes `operation_id`'s entry (if any) from `tab`'s pending-challenge
/// queue, then re-presents whichever challenge (if any) is now at the
/// front by populating `tab.password_dialog` for it -- or hides the
/// dialog if the queue is now empty.
///
/// Used both when an operation reaches a terminal state (below, and
/// `handle_extract_terminal`) and when the user answers/cancels the
/// currently-displayed challenge
/// (`password_management::presentation::ui::handle_password_dialogs`).
/// Either way, a second tab-scoped operation's still-queued challenge
/// (see `TabState::pending_challenge`'s own doc comment on why a tab
/// can have more than one operation in flight at once) must be shown
/// next rather than silently forgotten, which is exactly what the old
/// single-`Option` slot did: whichever challenge arrived first was
/// simply overwritten, and its waiter then hung forever.
pub(crate) fn dequeue_and_present_next(
    tab: &crate::core::tabs::TabState,
    operation_id: OperationId,
) {
    // `Signal::update` holds its write lock for the whole closure, so
    // the retain-then-inspect-front sequence below is atomic against
    // any other concurrent reader/writer of this same queue -- unlike
    // a `get()` snapshot, local mutation, then `set()`, which lets two
    // concurrent callers each read the same stale snapshot and one
    // silently clobber the other's write. This queue now has more than
    // one caller that can run concurrently on different threads/tasks:
    // the bridge's own event loop, the render thread (this function is
    // also called from `password_management::presentation::ui`'s
    // Unlock/Cancel handling), and -- since `register_operation`/
    // `reconcile_after_lag` can run `handle_event` (and therefore this)
    // directly on a caller's own task rather than only the bridge's
    // dedicated one -- potentially a third concurrent task as well.
    let mut next_challenge = None;
    tab.pending_challenge.update(|queue| {
        queue.retain(|pending| pending.operation_id != operation_id);
        next_challenge = queue.first().cloned();
    });
    let mut dialog = tab.password_dialog.get();
    match next_challenge {
        Some(next) => {
            dialog.show = true;
            dialog.password.clear();
            dialog.error = match &next.challenge {
                Challenge::Password { attempt, .. } if *attempt > 1 => {
                    "Incorrect password".to_string()
                }
                _ => String::new(),
            };
        }
        None => {
            dialog.show = false;
            dialog.error.clear();
        }
    }
    tab.password_dialog.set(dialog);
}

/// Drains any metadata [`buffer_or_apply_session_metadata`] already
/// buffered for `session_id` before this tab was stamped with it,
/// applying it to `tab` if given -- see `AppSignals::
/// pending_session_metadata`'s own doc comment for why that race
/// exists and how the two of them cooperate to close it.
///
/// Always drains regardless of whether `tab` is `Some`: a session whose
/// tab is already gone by the time this runs has no metadata
/// destination left, but the buffered entry must still be removed --
/// otherwise it leaks for the process's lifetime, since nothing else
/// will ever ask for it again. Idempotent: draining an
/// empty/already-drained entry is a harmless no-op, which is what
/// happens for the overwhelming majority of opens (no plugin calls
/// back with metadata before the tab is stamped).
///
/// Takes `&AppSignals` rather than `&SharedState`: the buffer lives on
/// signals alone, and this is called from `handle_open_archive_completed`
/// which already has a `&SharedState` in scope -- taking the narrower
/// type here just documents that nothing else about `shared` is needed.
fn apply_buffered_session_metadata(
    signals: &crate::core::signals::AppSignals,
    tab: Option<&crate::core::tabs::TabState>,
    session_id: arclain_app::ids::ArchiveSessionId,
) {
    let metadata = signals
        .pending_session_metadata
        .lock()
        .unwrap()
        .remove(&session_id);
    if let (Some(tab), Some(metadata)) = (tab, metadata) {
        tab.metadata.set(metadata);
    }
}

async fn handle_open_archive_completed(
    shared: &SharedState,
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
    snapshot: arclain_app::archive::ArchiveSnapshot,
) {
    // Always forget the origin, regardless of whether the tab lookup
    // below succeeds -- otherwise the origin map grows for the
    // process's lifetime for any tab closed mid-open.
    origins.forget(operation_id);

    let tab = shared.signals().tabs.get().get(tab_id).cloned();
    apply_buffered_session_metadata(shared.signals(), tab.as_deref(), snapshot.session_id);
    let Some(tab) = tab else {
        // The tab is gone -- most likely a cancel racing this very
        // completion (the tab was force-closed, which cancels any
        // in-flight open, but this operation had already reached
        // `Completed` and minted a session by the time that cancel was
        // recorded). Nothing will ever read `snapshot.session_id` off
        // any tab now; close it here rather than returning with it
        // leaked in the facade's session store forever.
        crate::core::operations::archive::close_archive_session(shared, Some(snapshot.session_id));
        return;
    };

    // Close whatever session this tab held *before* this one, if any --
    // the single choke point every successful open funnels through,
    // regardless of which call site triggered it. `start_archive_open`
    // is called directly on a tab that may already have an archive open
    // from several places with no `replace_active` in between to have
    // already discarded the old session along with its tab (toolbar
    // Open, Ctrl+O, opening a nested archive as the current one, opening
    // an extracted nested archive, and a content-password retry reopen
    // all reuse the same tab id) -- reading the old id here, at the one
    // place every open completion passes through, means no future call
    // site can reintroduce this leak by forgetting to close it first.
    let previous_session_id = tab.archive_session_id.get();
    if previous_session_id != Some(snapshot.session_id) {
        crate::core::operations::archive::close_archive_session(shared, previous_session_id);
    }

    tab.archive_session_id.set(Some(snapshot.session_id));
    tab.pending_open_operation.set(None);
    dequeue_and_present_next(&tab, operation_id);

    // Re-fetch this session's *current* metadata now that the tab is
    // stamped, rather than relying solely on `apply_buffered_session_
    // metadata` above. That drain only recovers a write whose
    // `SessionEvent` was actually handled (and therefore buffered) before
    // this point; a write whose event was instead dropped by a lagged
    // session-event broadcast is still sitting safely in the session
    // store (`ArchiveContextBridge` commits it before publishing) but
    // would otherwise never reach this tab at all, since
    // `reconcile_session_events_after_lag` only re-checks tabs *already*
    // stamped with a session id -- this tab had none until the line
    // above. Fetching fresh here closes that gap unconditionally: it is
    // a harmless no-op re-application on the overwhelmingly common path
    // (no metadata has landed yet), and the authoritative fix on the rare
    // one.
    if let Some(app) = shared.facade.clone() {
        apply_current_session_metadata(shared.signals(), &app, snapshot.session_id).await;
    }

    if let Err(error) = relist_for_browser_signals(shared, &tab, &snapshot.source_path).await {
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
}

fn handle_open_archive_failed_or_cancelled(
    shared: &SharedState,
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
    error: Option<arclain_app::error::ApplicationError>,
) {
    // Always forget regardless of whether the tab lookup below
    // succeeds -- see `handle_open_archive_completed`'s identical
    // ordering and its own doc comment. A failed/cancelled open never
    // reaches the point of minting a session id, so there is nothing
    // to drain from `pending_session_metadata` on this path (that
    // buffer is keyed by session id, and only a *successful* open ever
    // produces one).
    origins.forget(operation_id);

    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    tab.pending_open_operation.set(None);
    dequeue_and_present_next(&tab, operation_id);

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
}

/// Below this percent, a linear time-left extrapolation is withheld
/// entirely (see `handle_extract_progress`) -- too little progress to
/// extrapolate from produces a wildly noisy estimate, worse than no
/// estimate at all.
const MIN_PERCENT_FOR_TIME_ESTIMATE: u64 = 5;

/// Formats a `Duration` as a short, human-scale `MMm SSs` (or just
/// `SSs` under a minute) -- enough precision for a progress dialog,
/// without pulling in a duration-formatting dependency for two call
/// sites.
fn format_duration_short(duration: std::time::Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn handle_extract_progress(
    tab: &crate::core::tabs::TabState,
    percent: u64,
    message: Option<String>,
) {
    // `processed_text` deliberately stays untouched here: extraction's
    // `OperationState::Progress` reports `completed_units`/`total_units`
    // as a percent-out-of-100 (see `operations::extract::run_extract`'s
    // own `OperationState::Progress { completed_units: overall_percent,
    // total_units: Some(100), .. }`), not a real file/byte count.
    // Formatting that pair as "45/100" would just restate `percent` in
    // a second, redundant format next to the progress bar rather than
    // showing genuine per-item progress the way the pre-facade dialog's
    // `current`/`total` file counter did. Populating this field for
    // real would need the facade's extraction operation to report an
    // actual file counter -- a facade-level enhancement, not something
    // this bridge can fabricate from a percent alone.
    let mut dialog = tab.extraction_dialog().get();
    dialog.show = true;
    dialog.status = crate::shared::dialogs::ExtractionStatus::Running;
    let percent = percent.min(100);
    dialog.percent = percent as u8;
    if let Some(started_at) = dialog.started_at {
        let elapsed = started_at.elapsed();
        dialog.elapsed_text = format_duration_short(elapsed);
        // Simple linear extrapolation from "how long it took to reach
        // this percent" -- a rough estimate, not a precise ETA (7-Zip's
        // own per-file throughput varies too much for anything fancier
        // to be worth the complexity here), and only offered once
        // there's enough progress for the extrapolation to be
        // meaningful rather than wildly noisy.
        if (MIN_PERCENT_FOR_TIME_ESTIMATE..100).contains(&percent) {
            let total_estimated_secs = elapsed.as_secs_f64() * 100.0 / percent as f64;
            let remaining_secs = (total_estimated_secs - elapsed.as_secs_f64()).max(0.0);
            dialog.time_left_text =
                format_duration_short(std::time::Duration::from_secs_f64(remaining_secs));
        }
    }
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
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
    status: crate::shared::dialogs::ExtractionStatus,
    message: String,
) {
    // Always forget regardless of whether the tab lookup below
    // succeeds -- see `handle_open_archive_completed`'s identical
    // ordering and its own doc comment; the same leak applies here.
    origins.forget(operation_id);
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    let mut dialog = tab.extraction_dialog().get();
    dialog.status = status;
    dialog.show = false;
    tab.extraction_dialog().set(dialog);
    tab.active_extraction_operation.set(None);
    // An extraction can raise its own `Challenge::Password` (encrypted
    // entries); if this operation reaches a terminal state while that
    // challenge is still queued (e.g. the user cancelled instead of
    // answering it), the entry must not be left behind forever -- see
    // `dequeue_and_present_next`'s own doc comment.
    dequeue_and_present_next(&tab, operation_id);
    shared.signals().status_bar.update(|s| {
        s.message = message;
    });
}

/// Re-lists `path` directly through the backend selector to refresh
/// `tab`'s flat `entries`/`browser_entries` signals after a successful
/// archive mutation (`OperationKind::ArchiveModify` reaching
/// `Completed`) -- a deliberate cousin of `relist_for_browser_signals`
/// (used for a fresh `OpenArchive` completion), not a reuse of it: a
/// mutation never changes which archive is open, which folder the user
/// is viewing, or its encryption status, so none of `archive_extras`/
/// `navigation`/`current_password`/`opened_archive` are touched here,
/// unlike that function's own full reset.
///
/// Selection is pruned to just the paths still present in the fresh
/// listing rather than cleared outright: path-stable identity is what
/// the facade's own `EntryId` guarantees across a mutation-triggered
/// reindex (see `arclain_app::operations::archive_mutation`'s own doc
/// comment) -- pruning by path here is this flat, not-yet-`EntryId`-aware
/// browser model's practical equivalent of that guarantee, so deleting
/// one selected file does not also silently deselect every other one.
async fn refresh_entries_after_mutation(
    shared: &SharedState,
    tab: &crate::core::tabs::TabState,
    path: &Path,
) -> anyhow::Result<()> {
    let (backend, password) = {
        let state = shared.app_state.lock();
        (
            state.backend_selector.select(path)?,
            tab.current_password.get(),
        )
    };
    let path_owned = path.to_path_buf();
    // `backend.list()` is a real blocking filesystem/subprocess call --
    // see `resolve_archive_listing`'s identical concern for why this
    // must not run inline on the bridge's own event loop.
    let info = tokio::task::spawn_blocking(move || backend.list(&path_owned, password.as_deref()))
        .await
        .map_err(|join_error| anyhow::anyhow!("archive re-list task panicked: {join_error}"))??;

    let fresh_paths: std::collections::HashSet<String> = info
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    {
        let mut view_state = tab.browser_view_state.get();
        let stale: Vec<String> = view_state
            .selection
            .iter()
            .filter(|selected| !fresh_paths.contains(*selected))
            .cloned()
            .collect();
        let mut changed = false;
        for stale_path in stale {
            changed |= view_state.selection.remove(&stale_path);
        }
        if changed {
            tab.browser_view_state.set_if_changed(view_state);
        }
    }

    tab.entries.set(Arc::new(info.entries));
    crate::core::operations::navigation_view::refresh_view_entries_for_tab(
        shared.signals(),
        tab.id,
    );
    Ok(())
}

/// Refreshes `tab_id`'s browser signals once its `ArchiveModify`
/// operation reaches `Completed` -- see [`refresh_entries_after_mutation`].
/// A no-op if the tab (or its archive path) is already gone -- nothing
/// left to refresh.
///
/// Deliberately *not* also wired to the transient `SnapshotChanged`
/// state the worker emits immediately before `Completed`: an earlier
/// version of this bridge refreshed on both, which meant every
/// successful mutation triggered two full UI-side `backend.list()`
/// calls back to back (three counting the facade's own internal
/// re-list inside `run_archive_mutation` -- for a 7z-backed archive,
/// each is a subprocess spawn plus a full central-directory read) for
/// no benefit: the worker always emits `Completed` immediately after
/// `SnapshotChanged`, so there is no meaningful gap between them for a
/// live subscriber to observe one without the other. `Completed` alone
/// is also sufficient for the *reconciliation* path (`register_operation`'s
/// own one-shot catch-up, and `reconcile_after_lag`): both replay
/// whichever state the registry's record currently holds, which for a
/// finished operation is always the terminal one -- reconciliation
/// landing on `SnapshotChanged` specifically was never something either
/// path could rely on, since `OperationRegistry` keeps only the latest
/// state, not a history.
async fn refresh_tab_after_archive_modify(shared: &SharedState, tab_id: TabId) {
    let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() else {
        return;
    };
    let Some(archive_path) = tab.archive_path.get() else {
        return;
    };
    if let Err(error) = refresh_entries_after_mutation(shared, &tab, &archive_path).await {
        tracing::error!(
            "[operation_bridge] archive mutation succeeded via the facade but the UI-side \
             re-list failed: {error:#}"
        );
        shared.signals().status_bar.update(|status| {
            status.message = format!("Archive changed but failed to refresh the view: {error:#}");
        });
    }
}

/// Handles `OperationKind::ArchiveModify`'s `Completed` state: refreshes
/// the tab (see [`refresh_tab_after_archive_modify`]'s own doc comment
/// on why this does not rely solely on having already observed
/// `SnapshotChanged` live), reports the outcome, and forgets the
/// operation's tracked origin.
async fn handle_archive_modify_completed(
    shared: &SharedState,
    origins: &OperationOrigins,
    tab_id: TabId,
    operation_id: OperationId,
) {
    refresh_tab_after_archive_modify(shared, tab_id).await;
    origins.forget(operation_id);
    shared.signals().status_bar.update(|status| {
        status.message = "Archive updated".to_string();
    });
}

/// Handles `OperationKind::ArchiveModify`'s non-`Completed` terminal
/// states (`Cancelled`/`Failed`). Neither ever follows a `SnapshotChanged`
/// -- the worker only ever emits that once its mutation has already
/// succeeded, immediately before its own `Completed` transition, and a
/// terminal record can never be followed by another transition (see
/// `OperationRegistry::transition`'s own terminal-state invariant) -- so
/// there is nothing to refresh here; only the outcome is reported.
/// Unlike extraction/open, `ArchiveModify` has no dedicated per-tab
/// dialog or "operation in flight" signal to clear -- it is a
/// fire-and-forget mutation with no UI state of its own beyond the
/// status bar message.
fn handle_archive_modify_terminal(
    shared: &SharedState,
    origins: &OperationOrigins,
    operation_id: OperationId,
    message: String,
) {
    origins.forget(operation_id);
    shared.signals().status_bar.update(|status| {
        status.message = message;
    });
}

/// Resolves what a completed `Materialize` operation's [`MaterializationAction`]
/// says to do, and does it. Always forgets both `origins` and `actions`'
/// entries for `operation_id` first -- every path below is a leaf (nothing
/// re-dispatches another operation that would need this same entry again).
/// Takes no `TabId`: neither branch below needs one -- an external-open
/// launch is not scoped to any tab's UI, and a nested archive open (see
/// `crate::core::app_lifecycle::open_nested_archive_in_tab`'s own doc
/// comment) already targets whichever tab is active, matching the
/// pre-facade behavior exactly (it never threaded a specific origin tab
/// through either).
fn handle_materialize_completed(
    shared: &SharedState,
    origins: &OperationOrigins,
    actions: &MaterializationActions,
    operation_id: OperationId,
    lease: MaterializationLease,
) {
    origins.forget(operation_id);
    let Some(action) = actions.take(operation_id) else {
        // No pending action registered -- a materialize call this bridge
        // does not (yet) drive any UI behavior for. Nothing to do; the
        // lease itself is still valid and reachable by its id for whatever
        // did start it, if it kept the id some other way.
        return;
    };
    match action {
        MaterializationAction::ExternalOpen { relative_target } => {
            let target_path = match &relative_target {
                Some(relative) => lease.local_path.join(relative),
                None => lease.local_path.clone(),
            };
            let release_now = |shared: &SharedState, lease_id: MaterializationLeaseId| {
                if let Some(app) = shared.facade.clone() {
                    shared.services.tokio_runtime.clone().spawn(async move {
                        let _ = app.release_materialization(lease_id).await;
                    });
                }
            };

            if !target_path.exists() {
                tracing::warn!(
                    "[operation_bridge] materialized lease {:?} but the expected file {} is missing",
                    lease.id,
                    target_path.display()
                );
                shared.signals().status_bar.update(|s| {
                    s.message = "Extracted file not found".to_string();
                });
                release_now(shared, lease.id);
                return;
            }

            if arclain_core::features::organization::flatten::is_archive_extension(&target_path) {
                // Route through arclain's own archive-open flow instead of
                // the OS default handler (keeps nested-archive browsing
                // inside the app, and surfaces the password dialog if the
                // inner archive is itself encrypted).
                crate::core::app_lifecycle::open_nested_archive_in_tab(shared, &target_path);
                // The nested open reads this lease's bytes exactly once
                // (via a fresh `start_open_archive` `list()` call against
                // `target_path`); nothing keeps reading them afterward, so
                // release immediately rather than track this lease for
                // ongoing renewal.
                release_now(shared, lease.id);
            } else if crate::core::app_lifecycle::open_extracted_file_via_signals(
                shared,
                &target_path,
            ) {
                // Unlike the nested-archive case, an external, OS-launched
                // application's own read timing is unknowable from here --
                // see `ExternalOpenLeases`'s own doc comment for why this
                // keeps renewing instead of releasing.
                shared.external_open_leases.track(lease.id);
            } else {
                // The OS spawn itself failed (missing handler, permission
                // issue, etc.) -- nothing was ever launched to read this
                // lease, so there is nothing to keep it alive for. Release
                // it now rather than track-and-renew it forever on a
                // swallowed error.
                release_now(shared, lease.id);
            }
        }
    }
}

fn handle_materialize_failed_or_cancelled(
    shared: &SharedState,
    origins: &OperationOrigins,
    actions: &MaterializationActions,
    operation_id: OperationId,
    error: Option<arclain_app::error::ApplicationError>,
) {
    origins.forget(operation_id);
    actions.take(operation_id);
    let message = match error {
        Some(error) => {
            tracing::error!("[operation_bridge] materialization failed: {error:?}");
            format!("Failed to open file: {}", error.summary)
        }
        None => "Opening file cancelled".to_string(),
    };
    shared.signals().status_bar.update(|s| {
        s.message = message.clone();
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
    let attempt = *attempt;
    // `Signal::update` makes the is-empty check and the push atomic
    // against any other concurrent reader/writer of this queue -- see
    // `dequeue_and_present_next`'s own doc comment on why this queue in
    // particular now has more than one thread/task that can touch it
    // concurrently.
    let mut already_showing = false;
    tab.pending_challenge.update(|queue| {
        // If the queue is already non-empty, some other operation's
        // challenge is currently being shown -- this one waits its turn
        // rather than clobbering the visible dialog (see `TabState::
        // pending_challenge`'s own doc comment). It gets presented once
        // that earlier challenge is answered/cancelled/terminalized, via
        // `dequeue_and_present_next`.
        already_showing = !queue.is_empty();
        queue.push(super::tabs::PendingChallenge {
            operation_id,
            challenge,
        });
    });
    if !already_showing {
        let mut dialog = tab.password_dialog.get();
        dialog.show = true;
        dialog.password.clear();
        dialog.error = if attempt > 1 {
            "Incorrect password".to_string()
        } else {
            String::new()
        };
        tab.password_dialog.set(dialog);
    }
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

/// Routes one `OperationKind::PluginAction` event back to the plugin UI
/// slot that started it.
///
/// The slot registry decides whether the update belongs to the slot at
/// all -- see
/// `crate::features::plugins::application::facade_sessions::PluginSessions::apply_update`
/// for the three rejection cases. This function only handles the egui
/// side of an accepted one: the new document is already stored by
/// `apply_update`, so all that remains is running the intents that came
/// with it against the slot's owning tab, and repainting.
///
/// A failed action deliberately does not blank the slot: its last good
/// document is still valid and still worth drawing (see
/// `PluginSessions::fail`), so the failure surfaces as a toast instead.
pub fn handle_plugin_action_event(shared: &SharedState, event: OperationEvent) {
    use crate::features::plugins::presentation::document_dispatch;

    match event.state {
        OperationState::Completed {
            result: OperationResult::PluginUiUpdated { update },
        } => {
            let Some(applied) = shared
                .plugin_sessions
                .apply_update(event.operation_id, update)
            else {
                return;
            };
            let origin_tab = applied
                .slot
                .tab()
                .unwrap_or_else(|| shared.signals().tabs.get().active_id());
            document_dispatch::apply_intents(shared, &applied.slot, origin_tab, applied.intents);
            shared.signals().kick_repaint();
        }
        OperationState::Failed { error } => {
            if shared.plugin_sessions.fail(event.operation_id).is_some() {
                tracing::warn!("[operation_bridge] plugin action failed: {error:?}");
                shared.toaster.lock().error(error.summary);
                shared.signals().kick_repaint();
            }
        }
        OperationState::Cancelled => {
            let _ = shared.plugin_sessions.fail(event.operation_id);
        }
        // A plugin action carries no progress, raises no challenge, and
        // its only meaningful completion result is `PluginUiUpdated`.
        OperationState::Accepted
        | OperationState::Started
        | OperationState::Progress { .. }
        | OperationState::Challenge { .. }
        | OperationState::SnapshotChanged { .. }
        | OperationState::Completed { .. } => {}
    }
}

/// Handles one operation event, dispatching to the tab-specific handlers
/// above based on `event.kind`/`event.state`. `async` because the
/// `OpenArchive` completion path awaits `relist_for_browser_signals`,
/// whose own blocking backend call now runs on a `spawn_blocking` task
/// (see that function's doc comment) rather than inline on this loop.
async fn handle_event(
    shared: &SharedState,
    origins: &OperationOrigins,
    materialization_actions: &MaterializationActions,
    runtime: &tokio::runtime::Runtime,
    event: OperationEvent,
) {
    // Plugin actions are routed by slot, not by tab: their origin lives in
    // `shared.plugin_sessions`, not in `OperationOrigins` (a window-scoped
    // slot such as the toolbar's has no owning tab at all). Handled before
    // the `origins` lookup below, which would otherwise drop every one of
    // them as "not one of ours".
    if event.kind == OperationKind::PluginAction {
        handle_plugin_action_event(shared, event);
        return;
    }

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
                origins,
                tab_id,
                event.operation_id,
                crate::shared::dialogs::ExtractionStatus::Completed,
                "Extraction completed".to_string(),
            );
        }
        (OperationKind::Extract, OperationState::Cancelled) => {
            handle_extract_terminal(
                shared,
                origins,
                tab_id,
                event.operation_id,
                crate::shared::dialogs::ExtractionStatus::Cancelled,
                "Extraction cancelled".to_string(),
            );
        }
        (OperationKind::Extract, OperationState::Failed { error }) => {
            let message = format!("Extraction failed: {}", error.summary);
            handle_extract_terminal(
                shared,
                origins,
                tab_id,
                event.operation_id,
                crate::shared::dialogs::ExtractionStatus::Failed,
                message,
            );
        }
        (
            OperationKind::OpenArchive,
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            },
        ) => {
            handle_open_archive_completed(shared, origins, tab_id, event.operation_id, snapshot)
                .await
        }
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
        // `SnapshotChanged` deliberately has no handler of its own here:
        // see `refresh_tab_after_archive_modify`'s own doc comment for
        // why refreshing on it *in addition to* `Completed` doubled the
        // UI-side re-list (and the facade's own internal one) for every
        // successful mutation, since the worker always emits `Completed`
        // immediately after `SnapshotChanged` -- there is no meaningful
        // gap between them for a user to benefit from an earlier
        // refresh, and both `register_operation`'s own reconciliation
        // and `reconcile_after_lag` already land on whichever state is
        // *latest*, which for a finished mutation is always `Completed`.
        (OperationKind::ArchiveModify, OperationState::Completed { .. }) => {
            handle_archive_modify_completed(shared, origins, tab_id, event.operation_id).await;
        }
        (OperationKind::ArchiveModify, OperationState::Cancelled) => {
            handle_archive_modify_terminal(
                shared,
                origins,
                event.operation_id,
                "Archive change cancelled".to_string(),
            );
        }
        (OperationKind::ArchiveModify, OperationState::Failed { error }) => {
            let message = format!("Archive change failed: {}", error.summary);
            handle_archive_modify_terminal(shared, origins, event.operation_id, message);
        }
        (
            OperationKind::Materialize,
            OperationState::Completed {
                result: OperationResult::Materialized { lease },
            },
        ) => handle_materialize_completed(
            shared,
            origins,
            materialization_actions,
            event.operation_id,
            lease,
        ),
        (OperationKind::Materialize, OperationState::Cancelled) => {
            handle_materialize_failed_or_cancelled(
                shared,
                origins,
                materialization_actions,
                event.operation_id,
                None,
            )
        }
        (OperationKind::Materialize, OperationState::Failed { error }) => {
            handle_materialize_failed_or_cancelled(
                shared,
                origins,
                materialization_actions,
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
fn snapshot_to_event(snapshot: arclain_app::event::OperationSnapshot) -> OperationEvent {
    OperationEvent {
        operation_id: snapshot.operation_id,
        sequence: snapshot.last_sequence,
        kind: snapshot.kind,
        state: snapshot.state,
    }
}

/// Re-fetches `operation_id`'s current point-in-time snapshot directly
/// (bypassing the broadcast channel entirely) and replays it through the
/// exact same [`handle_event`] dispatch a live event would have gone
/// through -- catching up on whatever this bridge missed.
///
/// If the registry no longer has the operation at all (`Err`; it
/// finished and enough newer operations completed after it to evict its
/// history -- see `OperationRegistry::evict_excess_history`), there is
/// no state left to recover, but the origin must still be forgotten and
/// the tab's dialogs must still stop spinning -- both would otherwise
/// be stuck forever, since nothing will ever tell this bridge about
/// that operation again.
async fn reconcile_one(
    shared: &SharedState,
    origins: &OperationOrigins,
    runtime: &tokio::runtime::Runtime,
    app: &ArclainApp,
    operation_id: OperationId,
) {
    match app.operation(operation_id).await {
        Ok(snapshot) => {
            handle_event(
                shared,
                origins,
                &shared.materialization_actions,
                runtime,
                snapshot_to_event(snapshot),
            )
            .await;
        }
        Err(_) => {
            tracing::warn!(
                "[operation_bridge] lost track of operation {operation_id:?} -- its history was \
                 already evicted from the registry; forcing it to a terminal state so its tab \
                 does not spin forever"
            );
            if let Some(tab_id) = origins.resolve(operation_id) {
                if let Some(tab) = shared.signals().tabs.get().get(tab_id).cloned() {
                    let mut extraction_dialog = tab.extraction_dialog().get();
                    extraction_dialog.show = false;
                    tab.extraction_dialog().set(extraction_dialog);
                    tab.active_extraction_operation.set(None);
                    tab.pending_open_operation.set(None);
                    dequeue_and_present_next(&tab, operation_id);
                }
            }
            origins.forget(operation_id);
        }
    }
}

/// Registers `operation_id` as belonging to `tab_id`, then immediately
/// reconciles against its current snapshot.
///
/// Every `start_open_archive`/`start_extract` call site must use this
/// instead of calling `OperationOrigins::register` directly: the
/// operation's worker starts running (and can publish events,
/// including a terminal one for a fast-failing operation) concurrently
/// with the caller resuming from that `.await` and reaching the
/// registration call, and the bridge's own receiver task can observe
/// and drop such an event -- having no registered origin yet -- before
/// this ever runs. The one-shot reconciliation immediately afterward
/// catches up on whatever state the operation already reached before
/// registration landed, exactly the same way `reconcile_after_lag`
/// catches up after a dropped broadcast event. Replaying a state the
/// live event stream will *also* still deliver (the common case: the
/// operation is still `Accepted`/`Started`) is a harmless no-op --
/// every `handle_event` branch either re-applies the same values or
/// falls through the terminal-only catch-all.
pub async fn register_operation(shared: &SharedState, operation_id: OperationId, tab_id: TabId) {
    shared.operation_origins.register(operation_id, tab_id);
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();
    reconcile_one(
        shared,
        &shared.operation_origins,
        &runtime,
        &app,
        operation_id,
    )
    .await;
}

/// Called after the bridge's receiver reports `Lagged`: some number of
/// events were dropped from the broadcast channel before this worker
/// could read them, which can include a terminal event for an
/// operation this bridge is still tracking -- left alone, that tab's
/// dialog would spin forever and the leaked origin would never be
/// forgotten. Reconciles every currently-tracked operation directly
/// against its own snapshot (see `reconcile_one`), bypassing the lagged
/// channel entirely. `pub` (rather than the module-private default) so
/// integration tests can drive this reconciliation path directly,
/// without needing to manufacture an actual broadcast-channel overflow.
pub async fn reconcile_after_lag(
    shared: &SharedState,
    origins: &OperationOrigins,
    runtime: &tokio::runtime::Runtime,
    app: &ArclainApp,
    skipped: u64,
) {
    tracing::warn!(
        "[operation_bridge] the operation-event broadcast channel lagged, dropping {skipped} \
         event(s) -- reconciling every tracked operation directly"
    );
    for operation_id in origins.tracked_ids() {
        reconcile_one(shared, origins, runtime, app, operation_id).await;
    }
}

// ---------------------------------------------------------------------
// Session events: a second, independent stream from the same facade.
//
// A session-scoped change (currently: a plugin's metadata/rename write,
// via `arclain_app::plugins::ArchiveContextBridge`) carries only a
// session id, not an `OperationId` -- there is no `OperationOrigins`
// entry to resolve a tab through, and no per-call-site "registered
// before await" race the way `register_operation` exists to close (see
// its own doc comment): nothing here ever registers a session id in
// advance, so every session event is handled the same way whether it is
// the first one this worker has ever seen for that session or the
// thousandth. `buffer_or_apply_session_metadata` and
// `apply_current_session_metadata` are shared by both the live-event
// path ([`handle_session_event`]) and the lag-reconciliation path
// ([`reconcile_session_events_after_lag`]) below.
// ---------------------------------------------------------------------

/// Applies `metadata` and `source_path` to whichever tab currently holds
/// `session_id` open, or buffers `metadata` in `AppSignals::
/// pending_session_metadata` if no tab is stamped with that session id
/// yet -- see [`apply_buffered_session_metadata`]'s doc comment for the
/// race this buffering exists to close. The write itself already landed
/// in `arclain_app::plugins::ArchiveContextBridge`'s own session store
/// (this function only reacts to the [`SessionEvent`] announcing it),
/// entirely independent of whether any tab has been stamped with the
/// session id yet.
///
/// `source_path` is applied alongside `metadata`, not buffered
/// separately: `SessionEvent::MetadataChanged` also fires after a
/// plugin-triggered rename (`ArchiveContextBridge::set_archive_path`
/// mutates `source_path`, which is session-visible state through
/// `archive_snapshot` exactly like `metadata`), and this event carries no
/// payload distinguishing "a rename happened" from "metadata changed" --
/// every event means "go re-fetch the full current snapshot", so both
/// fields are always applied together. Renaming requires an archive
/// already open (`rename_archive`'s host function rejects the call
/// otherwise), so a rename can never reach the "no tab stamped yet"
/// branch in practice -- only `metadata` needs buffering there.
///
/// No match can mean the tab's session was already closed (nothing left
/// to update, the write is simply lost) or that `handle_open_archive_
/// completed` has not yet stamped the originating tab -- this function
/// cannot tell which case it's in, so it always buffers alongside
/// applying wherever a match already exists.
fn buffer_or_apply_session_metadata(
    signals: &crate::core::signals::AppSignals,
    session_id: arclain_app::ids::ArchiveSessionId,
    metadata: Option<serde_json::Value>,
    source_path: std::path::PathBuf,
) {
    let tab = signals
        .tabs
        .get()
        .tabs()
        .iter()
        .find(|tab| tab.archive_session_id.get() == Some(session_id))
        .cloned();
    match tab {
        Some(tab) => {
            tracing::debug!(
                "[operation_bridge] applied a SessionEvent::MetadataChanged for session \
                 {session_id:?} to its stamped tab"
            );
            tab.metadata.set(metadata);
            tab.archive_path.set(Some(source_path));
        }
        None => {
            let mut pending = signals.pending_session_metadata.lock().unwrap();
            // Bounded: see `MAX_PENDING_SESSION_METADATA`'s own doc
            // comment. `contains_key` first so a session already
            // buffered (a plugin reporting an updated guess before its
            // tab is stamped) can still update its own entry even once
            // the map is otherwise at capacity.
            if pending.len() >= MAX_PENDING_SESSION_METADATA && !pending.contains_key(&session_id) {
                tracing::warn!(
                    "[operation_bridge] pending_session_metadata is at its {} entry cap -- \
                     dropping metadata reported for session {session_id:?} instead of \
                     growing unbounded",
                    MAX_PENDING_SESSION_METADATA
                );
                return;
            }
            tracing::debug!(
                "[operation_bridge] buffered a SessionEvent::MetadataChanged for session \
                 {session_id:?} -- no tab is stamped with it yet"
            );
            pending.insert(session_id, metadata);
        }
    }
}

/// Re-fetches `session_id`'s current `archive_snapshot` and applies its
/// metadata and source path via [`buffer_or_apply_session_metadata`]. A
/// `NotFound` result (the session was already closed by the time this
/// runs) is a harmless no-op -- there is nothing left to reconcile.
async fn apply_current_session_metadata(
    signals: &crate::core::signals::AppSignals,
    app: &ArclainApp,
    session_id: arclain_app::ids::ArchiveSessionId,
) {
    let Ok(snapshot) = app.archive_snapshot(session_id).await else {
        return;
    };
    buffer_or_apply_session_metadata(signals, session_id, snapshot.metadata, snapshot.source_path);
}

/// Handles one [`SessionEvent`]. `async` because it re-fetches
/// `archive_snapshot` rather than trusting a payload the event itself
/// does not carry -- see `SessionEvent::MetadataChanged`'s own doc
/// comment for why the event stays payload-free by design. Takes
/// `&AppSignals` rather than `&SharedState`: like
/// [`apply_buffered_session_metadata`], nothing else about `SharedState`
/// is needed.
pub async fn handle_session_event(
    signals: &crate::core::signals::AppSignals,
    app: &ArclainApp,
    event: SessionEvent,
) {
    match event {
        SessionEvent::MetadataChanged { session_id } => {
            apply_current_session_metadata(signals, app, session_id).await;
        }
    }
}

/// Called after the session-event receiver reports `Lagged`. Unlike
/// [`reconcile_after_lag`] (which replays each *tracked operation's* own
/// current snapshot via [`OperationOrigins::tracked_ids`]), a session
/// event carries no operation id and nothing here pre-registers "which
/// sessions this worker cares about" -- every currently open tab with a
/// stamped `archive_session_id` is exactly that set instead. Re-fetching
/// each one's `archive_snapshot` catches up on any metadata or rename a
/// dropped event would have announced, since `archive_snapshot` always
/// returns the authoritative current state regardless of how many
/// intermediate events were missed in between. `pub` for the same reason
/// as `reconcile_after_lag`: integration tests drive this directly
/// rather than manufacturing a real channel overflow.
pub async fn reconcile_session_events_after_lag(
    signals: &crate::core::signals::AppSignals,
    app: &ArclainApp,
    skipped: u64,
) {
    tracing::warn!(
        "[operation_bridge] the session-event broadcast channel lagged, dropping {skipped} \
         event(s) -- reconciling every open tab's session directly"
    );
    let session_ids: Vec<arclain_app::ids::ArchiveSessionId> = signals
        .tabs
        .get()
        .tabs()
        .iter()
        .filter_map(|tab| tab.archive_session_id.get())
        .collect();
    for session_id in session_ids {
        apply_current_session_metadata(signals, app, session_id).await;
    }
}

/// Spawns the bridge worker onto `shared.services.tokio_runtime`. A no-op
/// if `shared.facade` is `None` (test fixtures that skip a full
/// `ArclainApp::bootstrap` -- see `SharedState::facade`'s own doc
/// comment). Reads `shared.operation_origins` directly -- the caller
/// (`SharedState::new`) constructs it before calling this, so every
/// clone of `shared` (including the one captured here) already shares
/// the same registry call sites register into.
///
/// Spawns *two* independent tasks, one per broadcast stream
/// (`subscribe_operations`/`subscribe_session_events`) rather than
/// `tokio::select!`-ing both on one loop: the two streams have unrelated
/// lag-reconciliation strategies (`OperationOrigins::tracked_ids` vs.
/// every open tab's stamped session id -- see [`reconcile_session_events_
/// after_lag`]'s own doc comment) and unrelated per-event state
/// (`OperationOrigins`/`MaterializationActions` vs. none at all), so
/// merging them into one `match` would only entangle two otherwise
/// independent concerns for no benefit -- neither stream's ordering
/// depends on the other's.
pub fn spawn(shared: &SharedState) {
    let Some(app) = shared.facade.clone() else {
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();

    let mut receiver = app.subscribe_operations();
    let operations_shared = shared.clone();
    let operations_app = app.clone();
    let operations_runtime = runtime.clone();
    runtime.clone().spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    handle_event(
                        &operations_shared,
                        &operations_shared.operation_origins,
                        &operations_shared.materialization_actions,
                        &operations_runtime,
                        event,
                    )
                    .await;
                    operations_shared.signals().kick_repaint();
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    reconcile_after_lag(
                        &operations_shared,
                        &operations_shared.operation_origins,
                        &operations_runtime,
                        &operations_app,
                        skipped,
                    )
                    .await;
                    operations_shared.signals().kick_repaint();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut session_receiver = app.subscribe_session_events();
    let session_shared = shared.clone();
    let session_app = app.clone();
    runtime.spawn(async move {
        loop {
            match session_receiver.recv().await {
                Ok(event) => {
                    handle_session_event(session_shared.signals(), &session_app, event).await;
                    session_shared.signals().kick_repaint();
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    reconcile_session_events_after_lag(
                        session_shared.signals(),
                        &session_app,
                        skipped,
                    )
                    .await;
                    session_shared.signals().kick_repaint();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::signals::AppSignals;
    use crate::core::tabs::{PendingChallenge, TabState};
    use arclain_app::challenge::Challenge;
    use arclain_app::ids::{ArchiveSessionId, ChallengeId};

    fn password_challenge(challenge_id: u64, attempt: u32) -> Challenge {
        Challenge::Password {
            id: ChallengeId::from_raw(challenge_id),
            archive_name: "archive.zip".to_string(),
            attempt,
        }
    }

    // -- handle_extract_progress: dialog reset + elapsed/time-left ----

    #[test]
    fn format_duration_short_uses_minutes_only_once_a_full_minute_has_passed() {
        assert_eq!(
            format_duration_short(std::time::Duration::from_secs(5)),
            "5s"
        );
        assert_eq!(
            format_duration_short(std::time::Duration::from_secs(59)),
            "59s"
        );
        assert_eq!(
            format_duration_short(std::time::Duration::from_secs(65)),
            "1m 5s"
        );
    }

    #[test]
    fn handle_extract_progress_computes_elapsed_and_time_left_from_started_at() {
        let tab = TabState::new(crate::core::tabs::TabId(1));
        {
            let mut dialog = tab.extraction_dialog().get();
            dialog.started_at =
                Some(std::time::Instant::now() - std::time::Duration::from_secs(10));
            tab.extraction_dialog().set(dialog);
        }

        handle_extract_progress(&tab, 50, None);

        let dialog = tab.extraction_dialog().get();
        assert_eq!(dialog.percent, 50);
        assert!(
            !dialog.elapsed_text.is_empty(),
            "elapsed_text must be computed once started_at is known"
        );
        assert!(
            !dialog.time_left_text.is_empty(),
            "time_left_text must be estimated once there is enough progress to extrapolate from"
        );
        // ~10s elapsed at 50% implies roughly another ~10s remaining --
        // a loose bound, not an exact one, since this is a rough
        // extrapolation by design.
        assert!(
            dialog.time_left_text.contains('s'),
            "expected a seconds-scale estimate, got {:?}",
            dialog.time_left_text
        );
    }

    #[test]
    fn handle_extract_progress_leaves_elapsed_and_time_left_blank_without_a_known_start_time() {
        let tab = TabState::new(crate::core::tabs::TabId(1));
        // started_at is None by default (ExtractionProgressDialog::default()).
        handle_extract_progress(&tab, 50, None);
        let dialog = tab.extraction_dialog().get();
        assert!(dialog.elapsed_text.is_empty());
        assert!(dialog.time_left_text.is_empty());
    }

    #[test]
    fn handle_extract_progress_does_not_estimate_time_left_before_five_percent() {
        // Extrapolating from a tiny sliver of progress produces a wildly
        // noisy estimate -- deliberately withheld until there is enough
        // signal to be worth showing at all.
        let tab = TabState::new(crate::core::tabs::TabId(1));
        {
            let mut dialog = tab.extraction_dialog().get();
            dialog.started_at = Some(std::time::Instant::now());
            tab.extraction_dialog().set(dialog);
        }
        handle_extract_progress(&tab, 1, None);
        assert!(tab.extraction_dialog().get().time_left_text.is_empty());
    }

    // -- apply_buffered_session_metadata ------------------------------

    #[test]
    fn metadata_reported_before_any_tab_is_stamped_is_buffered_not_dropped() {
        let signals = AppSignals::new();
        let session_id = ArchiveSessionId::from_raw(42);
        let metadata = Some(serde_json::json!({"game": "test"}));

        // No tab known yet for this session -- mirrors
        // `buffer_or_apply_session_metadata`'s own no-match branch
        // buffering rather than dropping.
        signals
            .pending_session_metadata
            .lock()
            .unwrap()
            .insert(session_id, metadata.clone());

        // Draining with no tab yet (the session's tab is still unknown)
        // must not lose the value -- there is nothing to apply it to,
        // but nothing has drained it either.
        assert_eq!(
            signals
                .pending_session_metadata
                .lock()
                .unwrap()
                .get(&session_id)
                .cloned(),
            Some(metadata)
        );
    }

    #[test]
    fn apply_buffered_session_metadata_applies_and_drains_once_a_tab_is_known() {
        let signals = AppSignals::new();
        let tab = TabState::new(crate::core::tabs::TabId(1));
        let session_id = ArchiveSessionId::from_raw(7);
        let metadata = Some(serde_json::json!({"game": "arrived-late"}));
        signals
            .pending_session_metadata
            .lock()
            .unwrap()
            .insert(session_id, metadata.clone());

        apply_buffered_session_metadata(&signals, Some(&tab), session_id);

        assert_eq!(
            tab.metadata.get(),
            metadata,
            "buffered metadata must land on the now-known tab"
        );
        assert!(
            signals
                .pending_session_metadata
                .lock()
                .unwrap()
                .get(&session_id)
                .is_none(),
            "a drained entry must not still be sitting in the buffer"
        );
    }

    #[test]
    fn apply_buffered_session_metadata_still_drains_when_no_tab_is_given() {
        // A session whose tab is already gone by the time this runs
        // (closed mid-open) has no metadata destination -- but the
        // buffered entry must still be removed, or it leaks for the
        // process's lifetime.
        let signals = AppSignals::new();
        let session_id = ArchiveSessionId::from_raw(9);
        signals
            .pending_session_metadata
            .lock()
            .unwrap()
            .insert(session_id, Some(serde_json::json!({"orphaned": true})));

        apply_buffered_session_metadata(&signals, None, session_id);

        assert!(
            signals
                .pending_session_metadata
                .lock()
                .unwrap()
                .get(&session_id)
                .is_none(),
            "draining with no tab must still remove the buffered entry"
        );
    }

    #[test]
    fn apply_buffered_session_metadata_is_a_harmless_no_op_when_nothing_is_buffered() {
        let signals = AppSignals::new();
        let tab = TabState::new(crate::core::tabs::TabId(1));
        apply_buffered_session_metadata(&signals, Some(&tab), ArchiveSessionId::from_raw(1));
        assert_eq!(tab.metadata.get(), None);
    }

    // -- buffer_or_apply_session_metadata --------------------------------

    /// A tab already stamped with the session id gets both the metadata
    /// and the source path applied directly -- no buffering involved.
    /// The `source_path` half is the fix for a plugin-triggered rename no
    /// longer reaching `tab.archive_path` after the bridge swap: this
    /// event fires for a rename too (see the function's own doc comment),
    /// and there is no way to tell "rename" and "metadata" events apart,
    /// so both fields are always applied together.
    #[test]
    fn buffer_or_apply_session_metadata_applies_directly_to_an_already_stamped_tab() {
        let signals = AppSignals::new();
        let tab = signals.tabs.get().active().clone();
        let session_id = ArchiveSessionId::from_raw(11);
        tab.archive_session_id.set(Some(session_id));

        buffer_or_apply_session_metadata(
            &signals,
            session_id,
            Some(serde_json::json!({"game": "stamped"})),
            std::path::PathBuf::from("renamed.zip"),
        );

        assert_eq!(
            tab.metadata.get(),
            Some(serde_json::json!({"game": "stamped"}))
        );
        assert_eq!(
            tab.archive_path.get(),
            Some(std::path::PathBuf::from("renamed.zip"))
        );
        assert!(
            signals
                .pending_session_metadata
                .lock()
                .unwrap()
                .get(&session_id)
                .is_none(),
            "a directly-applied write must not also sit in the pending buffer"
        );
    }

    /// No tab is stamped with the session id yet (mirrors a plugin's
    /// `OnArchiveOpen` handler calling back before `handle_open_archive_
    /// completed` gets around to stamping the originating tab) --
    /// buffered instead of dropped, exactly like the pre-swap bridge's
    /// own reasoning. Only `metadata` is bufferable; `source_path` is
    /// discarded in this branch on purpose -- a rename cannot reach it in
    /// practice (see the function's own doc comment).
    #[test]
    fn buffer_or_apply_session_metadata_buffers_when_no_tab_is_stamped_yet() {
        let signals = AppSignals::new();
        let session_id = ArchiveSessionId::from_raw(12);

        buffer_or_apply_session_metadata(
            &signals,
            session_id,
            Some(serde_json::json!({"game": "buffered"})),
            std::path::PathBuf::from("irrelevant.zip"),
        );

        assert_eq!(
            signals
                .pending_session_metadata
                .lock()
                .unwrap()
                .get(&session_id)
                .cloned(),
            Some(Some(serde_json::json!({"game": "buffered"})))
        );
    }

    /// Regression test mirroring the pre-swap bridge's own cap: this is a
    /// consumer of a broadcast a WASM plugin's write ultimately triggers,
    /// so a made-up or already-superseded session id must not grow this
    /// buffer forever.
    #[test]
    fn buffer_or_apply_session_metadata_does_not_grow_the_pending_buffer_past_its_cap() {
        let signals = AppSignals::new();
        for raw_id in 0..(MAX_PENDING_SESSION_METADATA as u64 + 10) {
            buffer_or_apply_session_metadata(
                &signals,
                ArchiveSessionId::from_raw(raw_id),
                Some(serde_json::json!({"n": raw_id})),
                std::path::PathBuf::from("fixture.zip"),
            );
        }
        assert!(
            signals.pending_session_metadata.lock().unwrap().len() <= MAX_PENDING_SESSION_METADATA
        );
    }

    /// Regression test restoring the coverage `AppSignalsBridge::
    /// set_session_metadata_still_updates_an_already_buffered_entry_once_at_capacity`
    /// had before the swap: a session id already buffered can still
    /// update its own entry even once the map is otherwise at its cap --
    /// only a *brand-new* id is rejected once full (see the function's
    /// own `contains_key` check).
    #[test]
    fn buffer_or_apply_session_metadata_still_updates_an_already_buffered_entry_once_at_capacity() {
        let signals = AppSignals::new();
        for raw_id in 0..MAX_PENDING_SESSION_METADATA as u64 {
            buffer_or_apply_session_metadata(
                &signals,
                ArchiveSessionId::from_raw(raw_id),
                Some(serde_json::json!({"n": raw_id})),
                std::path::PathBuf::from("fixture.zip"),
            );
        }
        assert_eq!(
            signals.pending_session_metadata.lock().unwrap().len(),
            MAX_PENDING_SESSION_METADATA
        );

        // Re-reporting for a session already buffered (e.g. a plugin
        // updating its own guess before the tab is stamped) must still go
        // through even though the map is at capacity -- only brand new
        // session ids are subject to the cap.
        buffer_or_apply_session_metadata(
            &signals,
            ArchiveSessionId::from_raw(0),
            Some(serde_json::json!({"n": "updated"})),
            std::path::PathBuf::from("fixture.zip"),
        );

        let pending = signals.pending_session_metadata.lock().unwrap();
        assert_eq!(pending.len(), MAX_PENDING_SESSION_METADATA);
        assert_eq!(
            pending.get(&ArchiveSessionId::from_raw(0)),
            Some(&Some(serde_json::json!({"n": "updated"})))
        );
    }

    // -- dequeue_and_present_next --------------------------------------

    #[test]
    fn a_second_challenge_is_queued_behind_the_first_rather_than_overwriting_it() {
        // Regression test: a tab's archive-open and its extraction are
        // independent operations that can both be in flight at once (see
        // `TabState::pending_challenge`'s own doc comment), and either
        // can raise its own `Challenge::Password`. With the old single
        // `Option` slot, the second challenge to arrive silently
        // overwrote the first, and the first operation's challenge
        // waiter then hung forever.
        let tab = TabState::new(crate::core::tabs::TabId(1));
        let op_a = OperationId::from_raw(1);
        let op_b = OperationId::from_raw(2);
        tab.pending_challenge.set(vec![
            PendingChallenge {
                operation_id: op_a,
                challenge: password_challenge(1, 1),
            },
            PendingChallenge {
                operation_id: op_b,
                challenge: password_challenge(2, 1),
            },
        ]);

        let queue = tab.pending_challenge.get();
        assert_eq!(
            queue.len(),
            2,
            "the second challenge must not have overwritten the first"
        );
        assert_eq!(queue[0].operation_id, op_a);
        assert_eq!(queue[1].operation_id, op_b);
    }

    #[test]
    fn dequeuing_the_front_challenge_presents_the_next_one_instead_of_hiding_the_dialog() {
        let tab = TabState::new(crate::core::tabs::TabId(1));
        let op_a = OperationId::from_raw(1);
        let op_b = OperationId::from_raw(2);
        tab.pending_challenge.set(vec![
            PendingChallenge {
                operation_id: op_a,
                challenge: password_challenge(1, 1),
            },
            PendingChallenge {
                operation_id: op_b,
                challenge: password_challenge(2, 1),
            },
        ]);

        dequeue_and_present_next(&tab, op_a);

        let queue = tab.pending_challenge.get();
        assert_eq!(
            queue.len(),
            1,
            "only the answered operation's entry must be removed"
        );
        assert_eq!(queue[0].operation_id, op_b);
        let dialog = tab.password_dialog.get();
        assert!(
            dialog.show,
            "a second operation's still-queued challenge must be presented, not silently dropped"
        );
    }

    #[test]
    fn dequeuing_the_only_challenge_hides_the_dialog() {
        let tab = TabState::new(crate::core::tabs::TabId(1));
        let op_a = OperationId::from_raw(1);
        tab.pending_challenge.set(vec![PendingChallenge {
            operation_id: op_a,
            challenge: password_challenge(1, 1),
        }]);
        {
            let mut dialog = tab.password_dialog.get();
            dialog.show = true;
            tab.password_dialog.set(dialog);
        }

        dequeue_and_present_next(&tab, op_a);

        assert!(tab.pending_challenge.get().is_empty());
        assert!(!tab.password_dialog.get().show);
    }

    #[test]
    fn dequeuing_an_operation_with_no_queued_challenge_is_a_harmless_no_op() {
        let tab = TabState::new(crate::core::tabs::TabId(1));
        let op_a = OperationId::from_raw(1);
        // Terminal-state cleanup calls this unconditionally (see
        // `handle_extract_terminal`) even when this operation never
        // raised a challenge at all.
        dequeue_and_present_next(&tab, op_a);
        assert!(tab.pending_challenge.get().is_empty());
    }

    #[test]
    fn concurrent_pending_challenge_mutations_never_lose_an_update() {
        // Regression test for the lost-update race a non-atomic
        // get()-modify-set() pair would exhibit: `pending_challenge` is
        // mutated from more than one thread/task in production now --
        // the bridge's own event loop, the render thread (`password_
        // management::presentation::ui`'s Unlock/Cancel handling calls
        // `dequeue_and_present_next` directly), and, since
        // `register_operation`/`reconcile_after_lag` can run
        // `handle_event` (and therefore `handle_password_challenge`/
        // `dequeue_and_present_next`) on a caller's own task rather than
        // only the bridge's dedicated one, potentially a third
        // concurrent task as well. `Signal::update` holding its write
        // lock across the whole closure is what makes the race
        // structurally unexpressible: this test drives real OS threads
        // hammering the same queue with no artificial delays, which
        // would make a get()-then-set() version of either mutation
        // reliably drop entries under this volume (2,000+ pushes across
        // 8 threads), not just occasionally.
        // Mirrors `handle_password_challenge`'s own push -- an `update`
        // closure, not a `get()`/`set()` pair. A plain nested `fn`
        // (rather than a closure) so it can be freely shared across the
        // `'static` thread closures below with no capture to worry about.
        fn push(tab: &Arc<TabState>, operation_id: OperationId) {
            tab.pending_challenge.update(|queue| {
                queue.push(PendingChallenge {
                    operation_id,
                    challenge: password_challenge(operation_id.into_raw(), 1),
                });
            });
        }

        let tab = Arc::new(TabState::new(crate::core::tabs::TabId(1)));
        const THREADS: u64 = 8;
        const PUSHES_PER_THREAD: u64 = 250;

        let pushers: Vec<_> = (0..THREADS)
            .map(|thread_index| {
                let tab = tab.clone();
                std::thread::spawn(move || {
                    for i in 0..PUSHES_PER_THREAD {
                        push(&tab, OperationId::from_raw(thread_index * 10_000 + i));
                    }
                })
            })
            .collect();
        for pusher in pushers {
            pusher.join().unwrap();
        }

        let total_pushed = (THREADS * PUSHES_PER_THREAD) as usize;
        assert_eq!(
            tab.pending_challenge.get().len(),
            total_pushed,
            "every concurrent push must survive -- a lost update would shrink this count"
        );

        // Now race real dequeues (the actual `dequeue_and_present_next`,
        // which needs no `SharedState`) against more concurrent pushes.
        const DEQUEUED: usize = 500;
        let dequeue_targets: Vec<OperationId> = tab
            .pending_challenge
            .get()
            .iter()
            .take(DEQUEUED)
            .map(|p| p.operation_id)
            .collect();
        let tab_for_dequeue = tab.clone();
        let dequeuer = std::thread::spawn(move || {
            for operation_id in dequeue_targets {
                dequeue_and_present_next(&tab_for_dequeue, operation_id);
            }
        });
        let tab_for_more_pushes = tab.clone();
        let more_pusher = std::thread::spawn(move || {
            for i in 0..PUSHES_PER_THREAD {
                push(&tab_for_more_pushes, OperationId::from_raw(999_000 + i));
            }
        });
        dequeuer.join().unwrap();
        more_pusher.join().unwrap();

        let expected = total_pushed - DEQUEUED + PUSHES_PER_THREAD as usize;
        assert_eq!(
            tab.pending_challenge.get().len(),
            expected,
            "concurrent dequeues and pushes must account for exactly what was removed and \
             added -- a lost update would leave stray entries or drop legitimate ones"
        );
    }

    // -- OperationOrigins::tracked_ids ----------------------------------

    #[test]
    fn tracked_ids_reflects_every_registered_operation_until_forgotten() {
        let origins = OperationOrigins::new();
        let op_a = OperationId::from_raw(1);
        let op_b = OperationId::from_raw(2);
        origins.register(op_a, TabId(1));
        origins.register(op_b, TabId(2));

        let mut ids = origins.tracked_ids();
        ids.sort_by_key(|id| id.into_raw());
        assert_eq!(ids, vec![op_a, op_b]);

        origins.forget(op_a);
        assert_eq!(origins.tracked_ids(), vec![op_b]);
    }

    // -- resolve_archive_listing (auto-password re-derivation) ---------
    //
    // Regression test for a bug the automated suite never caught during
    // Task 6 -- only a manual, real-archive test did: `resolve_archive_
    // listing` must independently re-derive the same auto-detected
    // password the facade's own `archive_ops::attempt_initial` resolved
    // internally. There is nothing to read the facade's own resolved
    // password back from without reaching into its private
    // `ArchiveSession` internals, which this crate must not do. Without
    // this, an archive a matching `PassRule` auto-unlocked on the facade
    // side (no password ever typed, no challenge ever raised) would open
    // successfully there while the UI-side re-list still saw an
    // encrypted, unreadable listing.

    struct FakeBackend {
        correct_password: String,
    }

    impl arclain_core::ArchiveBackend for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }
        fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
            arclain_core::archive::BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
            Ok(arclain_core::archive::ArchiveKind::Zip)
        }
        fn list(
            &self,
            _path: &Path,
            password: Option<&str>,
        ) -> anyhow::Result<arclain_core::archive::ArchiveInfo> {
            match password {
                Some(candidate) if candidate == self.correct_password => {
                    Ok(arclain_core::archive::ArchiveInfo {
                        archive_path: PathBuf::new(),
                        archive_kind: arclain_core::archive::ArchiveKind::Zip,
                        entries: vec![arclain_core::archive::ArchiveEntry {
                            path: "secret.txt".to_string(),
                            size: 10,
                            packed_size: 5,
                            modified: None,
                            is_dir: false,
                            encrypted: true,
                            crc32: None,
                        }],
                        encrypted: true,
                        headers_encrypted: false,
                        encryption_method: Some("AES256".to_string()),
                    })
                }
                _ => Err(anyhow::anyhow!("Wrong password for archive")),
            }
        }
        fn extract_all(&self, _: &Path, _: &Path, _: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_files(
            &self,
            _: &Path,
            _: &Path,
            _: &[String],
            _: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_directory(
            &self,
            _: &Path,
            _: &Path,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn recompress_7z(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_files(&self, _: &Path, _: &[PathBuf]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn create_archive(&self, _: &Path, _: &[PathBuf], _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn read_text_file(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
            unimplemented!()
        }
        fn delete_files(&self, _: &Path, _: &[String]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_or_update_file_from_str(&self, _: &Path, _: &str, _: &str) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn convert_to_7z(
            &self,
            _: &arclain_core::Archive,
            _: &Path,
            _: &Path,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn crc32_of_entry(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
            unimplemented!()
        }
    }

    fn matching_rule(archive_name: &str, password: &str) -> arclain_core::utilities::PassRule {
        arclain_core::utilities::PassRule {
            name: "test".to_string(),
            pattern: archive_name.to_string(),
            password: password.to_string(),
            priority: 10,
            enabled: true,
        }
    }

    #[test]
    fn resolve_archive_listing_auto_detects_a_password_from_a_matching_pass_rule() {
        let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeBackend {
            correct_password: "correct-horse-battery-staple".to_string(),
        });
        let rules = vec![matching_rule(
            "auto-unlock-fixture.zip",
            "correct-horse-battery-staple",
        )];
        let path = PathBuf::from("auto-unlock-fixture.zip");

        let resolved = resolve_archive_listing(backend, rules, Vec::new(), path, None)
            .expect("a matching pass rule must let this succeed without ever prompting");

        assert_eq!(
            resolved.resolved_password.as_deref(),
            Some("correct-horse-battery-staple")
        );
        assert_eq!(resolved.info.entries.len(), 1);
    }

    #[test]
    fn resolve_archive_listing_fails_when_no_pass_rule_matches_and_no_password_is_known() {
        let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeBackend {
            correct_password: "secret".to_string(),
        });
        let path = PathBuf::from("unmatched.zip");

        let result = resolve_archive_listing(backend, Vec::new(), Vec::new(), path, None);

        assert!(
            result.is_err(),
            "with no known password and no matching rule, this must fail rather than silently \
             succeeding with an unreadable listing"
        );
    }

    #[test]
    fn resolve_archive_listing_uses_an_already_known_password_without_consulting_pass_rules() {
        let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeBackend {
            correct_password: "already-known".to_string(),
        });
        let path = PathBuf::from("archive.zip");

        let resolved = resolve_archive_listing(
            backend,
            Vec::new(),
            Vec::new(),
            path,
            Some("already-known".to_string()),
        )
        .expect("an already-known correct password must succeed with no pass rules needed");

        assert_eq!(resolved.resolved_password.as_deref(), Some("already-known"));
    }
}

#[cfg(test)]
mod external_open_leases_tests {
    use super::ExternalOpenLeases;
    use arclain_app::ids::MaterializationLeaseId;
    use std::time::Duration;

    /// Regression test for the fix in `renew_due_external_open_leases`
    /// that marks every due lease renewed *before* spawning its actual
    /// (async) renewal call, not after that call completes: without
    /// that ordering, a lease found "due" by one per-frame call would
    /// still read as due on every subsequent frame until the previously
    /// spawned renewal task actually finished -- spawning a duplicate,
    /// redundant renewal call for the same lease on every intervening
    /// frame, for what is logically one "it's due" event. Uses small
    /// millisecond thresholds rather than the real 60-second production
    /// interval (`due_for_renewal` takes its threshold as a parameter,
    /// so this needs no clock injection and no real wait).
    #[test]
    fn a_lease_marked_renewed_is_not_immediately_due_again_at_the_same_threshold() {
        let leases = ExternalOpenLeases::new();
        let id = MaterializationLeaseId::from_raw(1);
        leases.track(id);

        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(
            leases.due_for_renewal(Duration::from_millis(10)),
            vec![id],
            "sanity: the lease is due once the threshold elapses"
        );

        // Mirrors `renew_due_external_open_leases`'s fix: mark renewed
        // synchronously, before the (here, simulated) async renewal
        // call would even start.
        leases.mark_renewed(id);

        assert!(
            leases.due_for_renewal(Duration::from_millis(10)).is_empty(),
            "a lease just marked renewed must not immediately read as due again at the \
             same threshold -- this is exactly the gap the old mark-after-completion \
             ordering left open for a duplicate per-frame renewal spawn"
        );
    }

    #[test]
    fn forgetting_a_lease_stops_it_from_ever_being_reported_as_due() {
        let leases = ExternalOpenLeases::new();
        let id = MaterializationLeaseId::from_raw(7);
        leases.track(id);
        leases.forget(id);

        std::thread::sleep(Duration::from_millis(5));
        assert!(
            leases.due_for_renewal(Duration::from_millis(1)).is_empty(),
            "a forgotten lease must never be reported as due"
        );
    }
}
