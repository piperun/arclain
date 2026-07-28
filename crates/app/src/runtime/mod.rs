//! The application root: owns the Tokio runtime and every composed
//! headless service, and is the facade both the egui frontend and a
//! future Flutter frontend construct instead of reaching into
//! `arclain_core`/`arclain_plugins` internals directly.
//!
//! # Runtime and executor rules
//!
//! [`ArclainApp::bootstrap`] is synchronous and constructs the
//! application-owned Tokio runtime (see [`bootstrap::run`]). Every
//! internal `spawn`/`spawn_blocking`/timer goes through the stored
//! [`AppRuntime::tokio_runtime`] handle, never the caller's ambient
//! runtime -- `capabilities()`/`health()`/`shutdown()` all dispatch
//! through `self.inner.tokio_runtime.handle()` for exactly this reason,
//! even though today's computation inside them is trivial enough that
//! it would "work" either way. Facade futures are executor-agnostic as
//! a result: a caller may await them from any runtime (the CLI's,
//! flutter_rust_bridge's, or egui's async integration) -- see
//! `crates/app/tests/bootstrap.rs`'s foreign-runtime tests.
//!
//! Async consumers should call [`ArclainApp::bootstrap`] (synchronous)
//! via `spawn_blocking`. [`ArclainApp::shutdown`] is itself `async` --
//! simply `.await` it directly; nothing in this crate creates a nested
//! Tokio runtime from inside an async context either way.

mod archive_ops;
mod bootstrap;
mod paths;
mod processing_ops;
mod session_store;

pub use bootstrap::BootstrapConfig;
pub use paths::AppPaths;
pub use session_store::{
    AppCapabilities, BackendCapabilityDto, ExternalToolStatusDto, HealthSnapshot, LegacyComposition,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arclain_core::backends::BackendSelector;
use arclain_core::utilities::PassRule;
use arclain_core::ArchiveBackend;
use arclain_plugins::PluginEventScheduler;

use crate::archive::ArchiveSnapshot;
use crate::archive::{ArchiveSessionStore, EntryPage, ListEntriesRequest, OpenArchiveRequest};
use crate::challenge::ChallengeResponse;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationEvent, OperationKind, OperationSnapshot};
use crate::ids::{ArchiveSessionId, OperationId};
use crate::operations::{
    ChallengeWaiters, ConvertRequest, OperationRegistry, OrganizeRequest, PipelineRequest,
};
use session_store::SessionStore;

/// Wraps the application's `Arc<tokio::runtime::Runtime>` so dropping
/// it never runs into `tokio::runtime::Runtime`'s ordinary blocking
/// `Drop` -- which panics ("Cannot drop a runtime in a context where
/// blocking is not allowed") if it executes from inside a task running
/// on that very runtime. `ArclainApp::dispatch` can create exactly that
/// scenario: it moves a clone of `Arc<AppRuntime>` into a task spawned
/// on the app's own runtime; if a caller drops the returned future
/// before it resolves and then drops its last `ArclainApp` clone, the
/// spawned task's own copy can end up being the last live reference,
/// dropped from one of the runtime's own worker threads.
///
/// The runtime is also not *exclusively* owned by this wrapper: it is
/// handed to `arclain_core::services::Services` as a plain
/// `Arc<tokio::runtime::Runtime>` too (`Services::tokio_runtime`,
/// `SessionStore::core_services` -- a type this crate cannot change),
/// and that clone can outlive this `RuntimeOwner` entirely once
/// `ArclainApp::take_legacy_composition` hands a `Services` off to
/// `crates/ui`. `Drop` (and [`Self::shutdown_now`]) account for this:
/// reclaim sole ownership via `Arc::try_unwrap` and shut down through
/// `shutdown_background` (signals shutdown and returns immediately --
/// safe from any context, including one of the runtime's own worker
/// threads) if this *is* the last reference; otherwise dropping our
/// own clone is an ordinary, always-safe reference-count decrement,
/// since only the drop that actually brings the count to zero can
/// reach `tokio::runtime::Runtime`'s real teardown logic at all.
///
/// This only fully protects `AppRuntime`'s *own* lifecycle, which is
/// why `AppRuntime` declares this field last (see its own doc
/// comment): as long as this `RuntimeOwner` is the one whose drop
/// observes the refcount reaching zero, every other clone dropping
/// first (including `SessionStore::core_services`'s own bare one) is
/// always safe, being a plain decrement. If `take_legacy_composition`
/// has handed a `Services` clone off to `crates/ui` and that clone
/// outlives every `ArclainApp` clone, its *own*, later drop is outside
/// this wrapper's reach entirely -- a pre-existing limitation of
/// `arclain_core::services::Services` holding an unwrapped
/// `Arc<Runtime>`, not something introduced or fixable here.
struct RuntimeOwner(parking_lot::Mutex<Option<Arc<tokio::runtime::Runtime>>>);

impl RuntimeOwner {
    fn new(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self(parking_lot::Mutex::new(Some(runtime)))
    }

    /// A `Handle` for spawning onto this runtime, or `None` once
    /// [`Self::shutdown_now`] (explicit or via `Drop`) has already run.
    fn handle(&self) -> Option<tokio::runtime::Handle> {
        self.0
            .lock()
            .as_ref()
            .map(|runtime| runtime.handle().clone())
    }

    /// Attempts an *eager* shutdown, but never gives up this wrapper's
    /// own protective reference to do it: on `Arc::try_unwrap` failure
    /// (something else -- typically `SessionStore::core_services`'s own
    /// bare clone, which is alive for as long as `AppRuntime` itself is
    /// -- is still using the runtime), the `Arc` is put *back* rather
    /// than dropped. Getting this wrong is exactly what an earlier
    /// version of this method did: it took the `Option`, and on
    /// `try_unwrap` failure just let the returned `Arc` drop at the end
    /// of the match arm -- a safe *decrement*, but one that permanently
    /// left this wrapper holding `None`. From that point on, `session`'s
    /// still-live, still-unwrapped clone became the *only* remaining
    /// reference, so whichever drop happened to release it next --
    /// including, after an explicit [`ArclainApp::shutdown`] call
    /// followed by dropping the app from inside an async context, one
    /// running on a worker thread -- reached `tokio::runtime::Runtime`'s
    /// unprotected `Drop` directly and could panic. Putting the `Arc`
    /// back preserves the invariant `AppRuntime`'s field order exists to
    /// establish: this wrapper is *always* still in the running to be
    /// the one whose own eventual `Drop` (guaranteed to run after every
    /// other runtime-owning field, see `AppRuntime`'s doc comment)
    /// observes the true last reference, however many times
    /// `shutdown_now` is called or fails to reclaim sole ownership
    /// along the way.
    ///
    /// Idempotent: once this call (or an earlier one, or `Drop`) *has*
    /// reclaimed and shut down the runtime, the `Option` is `None` and
    /// every further call is a no-op.
    fn shutdown_now(&self) {
        let Some(runtime_arc) = self.0.lock().take() else {
            return;
        };
        match Arc::try_unwrap(runtime_arc) {
            Ok(runtime) => runtime.shutdown_background(),
            Err(still_shared) => {
                // Not the last reference yet -- put it back so this
                // wrapper remains armed to observe whichever drop
                // eventually is the last one, rather than relinquishing
                // its own clone and leaving an unprotected one as the
                // sole survivor.
                *self.0.lock() = Some(still_shared);
            }
        }
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

/// Owns the Tokio runtime and every composed headless service for one
/// running application instance. Never constructed directly -- see
/// [`ArclainApp::bootstrap`].
///
/// Field order is load-bearing: Rust drops a struct's fields in
/// declaration order, and `session` embeds
/// `arclain_core::services::Services::tokio_runtime` -- a second,
/// *unwrapped* `Arc<tokio::runtime::Runtime>` clone this crate cannot
/// change the type of. `tokio_runtime` (this struct's own
/// [`RuntimeOwner`]) is declared *last of all fields, with no exception*
/// so it is always the one to observe (via `Arc::try_unwrap` in
/// [`RuntimeOwner::shutdown_now`]) whichever reference actually turns
/// out to be the final one, rather than `session`'s bare clone dropping
/// after it and reaching `tokio::runtime::Runtime`'s unprotected `Drop`
/// directly. `shut_down` holds no runtime reference at all (an
/// `AtomicBool`), so its own position wouldn't matter for this
/// invariant either way -- it is placed *before* `tokio_runtime`
/// anyway, precisely so "declared last" stays literally true of the one
/// field this invariant is actually about, with nothing to double-check
/// or explain away when reading the struct top to bottom. This fully
/// covers `AppRuntime`'s own lifecycle; see `RuntimeOwner`'s doc comment
/// for the one further-out case (a `Services` clone that escapes via
/// `take_legacy_composition` and outlives every `ArclainApp` clone)
/// this cannot reach.
pub(crate) struct AppRuntime {
    paths: AppPaths,
    session: SessionStore,
    /// The cancellable, event-broadcasting registry every asynchronous
    /// application operation (starting with `start_open_archive`) is
    /// tracked through.
    operations: OperationRegistry,
    /// Every open archive session, keyed by the `ArchiveSessionId` this
    /// store itself mints.
    archive_sessions: ArchiveSessionStore,
    /// Delivers a `ChallengeResponse`'s live payload to whichever
    /// operation task is waiting on it -- see the module's own doc
    /// comment for why this is a separate structure from `operations`.
    challenges: ChallengeWaiters,
    /// Test-only seam: when set, every archive open uses this backend
    /// instead of `SessionStore::backend_selector`'s real, extension-based
    /// selection -- lets integration tests exercise the full
    /// `start_open_archive` flow (challenges, retries, cancellation)
    /// against a deterministic fake backend without depending on a real
    /// encrypted archive fixture. Always `None` outside tests: production
    /// `BootstrapConfig::system_default()` never sets it.
    archive_backend_override: Option<Arc<dyn ArchiveBackend>>,
    /// The extraction operation's CLI-spawning seam (see
    /// `crate::operations::extract::ExtractRunner`). Always set, unlike
    /// `archive_backend_override`: there is no separate "real" code path
    /// to fall back to when no test override is given, so `bootstrap::run`
    /// always installs either the configured override or a real
    /// `SevenZipRunner`.
    extract_runner: Arc<dyn crate::operations::extract::ExtractRunner>,
    /// Set once by [`ArclainApp::shutdown`]. Every [`ArclainApp::dispatch`]
    /// call checks this first so a clone that outlives shutdown (held by
    /// another part of the program, or racing a concurrent shutdown call)
    /// gets a structured error instead of silently spawning onto a runtime
    /// that may already be tearing down. Also gates
    /// [`ArclainApp::take_legacy_composition`], so a post-shutdown caller
    /// cannot obtain a live `Services` either.
    shut_down: AtomicBool,
    tokio_runtime: RuntimeOwner,
}

impl AppRuntime {
    pub(crate) fn operations(&self) -> &OperationRegistry {
        &self.operations
    }

    pub(crate) fn archive_sessions(&self) -> &ArchiveSessionStore {
        &self.archive_sessions
    }

    pub(crate) fn challenges(&self) -> &ChallengeWaiters {
        &self.challenges
    }

    /// A `Handle` for spawning onto this app's own runtime, or `None`
    /// once [`ArclainApp::shutdown`] has actually reclaimed and torn it
    /// down (see `RuntimeOwner`'s doc comment -- in practice this only
    /// happens once every other reference, including `SessionStore::
    /// core_services`'s own bare clone, is also gone). Callers that
    /// reach this from inside a task already running on the app's own
    /// runtime (every `archive_ops` worker does) can treat `None` as
    /// "nothing left to do" and stop, the same way they already treat a
    /// closed challenge channel.
    pub(crate) fn tokio_handle(&self) -> Option<tokio::runtime::Handle> {
        self.tokio_runtime.handle()
    }

    /// Cheap: `BackendSelector` is a single `String` internally.
    pub(crate) fn backend_selector(&self) -> BackendSelector {
        self.session.backend_selector.clone()
    }

    pub(crate) fn archive_backend_override(&self) -> Option<Arc<dyn ArchiveBackend>> {
        self.archive_backend_override.clone()
    }

    pub(crate) fn extract_runner(&self) -> Arc<dyn crate::operations::extract::ExtractRunner> {
        self.extract_runner.clone()
    }

    pub(crate) fn pass_rules(&self) -> Vec<PassRule> {
        self.session.pass_rules.clone()
    }

    pub(crate) fn plugin_event_scheduler(&self) -> Option<PluginEventScheduler> {
        self.session.plugin_event_scheduler.clone()
    }

    // ---- Task 9: processing operations (Convert/Organize/Pipeline) ----
    // Everything in this section is this task's own addition; kept
    // together so a concurrent task touching this same shared file has a
    // small, predictable diff to merge around.

    /// The app's composed headless services (`arclain_core::services::
    /// Services`) -- `runtime::processing_ops::build_pipeline_context`
    /// reads `organization_service`/`library_service`/`config_service`/
    /// `config_db` off this the same way `crates/ui`'s pre-facade
    /// `process_runner.rs` already did off its own `Arc<Services>`
    /// handle.
    pub(crate) fn core_services(&self) -> &Arc<arclain_core::services::Services> {
        &self.session.core_services
    }

    /// The on-disk directories this instance resolved at bootstrap --
    /// `paths().config_dir` is where `start_pipeline` reads/resolves the
    /// saved-presets file (see `runtime::processing_ops::
    /// resolve_preset_pipeline`'s own doc comment for why this, and not
    /// `arclain_core::default_presets_path()`, is the correct path to
    /// use: the latter bypasses `paths_override` entirely and has its
    /// own directory-creation side effect).
    pub(crate) fn paths(&self) -> &AppPaths {
        &self.paths
    }
}

/// The application facade. Cheap to clone (an `Arc` internally); every
/// clone refers to the same running application instance.
///
/// This is the crate's headline export: the entire point of Stage 1's
/// facade extraction is that a frontend depends on `arclain_app` and
/// this type instead of `arclain_core`/`arclain_plugins` directly.
#[derive(Clone)]
pub struct ArclainApp {
    inner: Arc<AppRuntime>,
}

/// Hand-written rather than `#[derive(Debug)]`: `AppRuntime` holds
/// `arclain_core`/`arclain_plugins` service types that don't (and mostly
/// shouldn't) implement `Debug` themselves. `paths` is the one field
/// that is both `Debug` and actually useful to see in a panic message
/// (for example from `Result::expect`/`expect_err` in a test).
impl std::fmt::Debug for ArclainApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArclainApp")
            .field("paths", &self.inner.paths)
            .finish_non_exhaustive()
    }
}

impl ArclainApp {
    /// Synchronously constructs the application: resolves and creates
    /// its on-disk directories, builds the Tokio runtime it will own for
    /// its lifetime, and composes every headless service in the order
    /// `crates/ui/src/core/state/init.rs` used to (see
    /// [`bootstrap::run`] for the full sequence and what changed moving
    /// it here).
    pub fn bootstrap(config: BootstrapConfig) -> Result<Self, ApplicationError> {
        let runtime = bootstrap::run(config)?;
        Ok(Self {
            inner: Arc::new(runtime),
        })
    }

    /// The directories this instance resolved at bootstrap.
    pub fn paths(&self) -> &AppPaths {
        &self.inner.paths
    }

    /// What this running application can actually do right now --
    /// which archive formats support which operations, and whether the
    /// external tools/plugins they may depend on are present.
    pub async fn capabilities(&self) -> Result<AppCapabilities, ApplicationError> {
        self.dispatch(|inner| inner.session.capabilities()).await
    }

    /// A coarse liveness/readiness signal.
    pub async fn health(&self) -> Result<HealthSnapshot, ApplicationError> {
        self.dispatch(|inner| inner.session.health()).await
    }

    /// Explicit, idempotent shutdown. The first call marks this
    /// instance (and every clone of it -- the flag lives in the shared
    /// `AppRuntime`) shut down, and *attempts* an eager teardown of the
    /// application's Tokio runtime via [`RuntimeOwner::shutdown_now`].
    /// "Attempts" is precise, not "does": a real bootstrapped app always
    /// has `SessionStore::core_services` (`arclain_core::services::
    /// Services::tokio_runtime`) holding its own bare clone for as long
    /// as `AppRuntime` itself is alive, so this call almost never
    /// actually reaches `shutdown_background` -- and that is by design,
    /// not a bug: forcing the runtime down while that clone (or a
    /// `Services` handed out through [`Self::take_legacy_composition`])
    /// is still in use would be unsound regardless of which drop
    /// happened to trigger it. The runtime's *real* teardown happens
    /// whenever the true last reference is actually released, wherever
    /// that occurs, observed by `RuntimeOwner`'s own `Drop` -- which
    /// `shutdown_now` never gives up the ability to do (see its doc
    /// comment). What `shutdown()` *does* unconditionally and
    /// immediately do is make the application unusable: every
    /// subsequent facade call on any clone -- including a second
    /// `shutdown()` call racing this one, and including a clone the
    /// caller kept around after this one shut the application down --
    /// goes through [`Self::dispatch`] (or, for
    /// [`Self::take_legacy_composition`], its own equivalent check),
    /// which checks the shutdown flag first: a second `shutdown()` call
    /// is a documented no-op success (returns `Ok(())` without doing
    /// anything further), while every *other* facade method returns a
    /// structured `ApplicationError` (`kind: Internal`).
    pub async fn shutdown(&self) -> Result<(), ApplicationError> {
        if self.inner.shut_down.swap(true, Ordering::SeqCst) {
            // Already shut down (by an earlier call on this clone or
            // any other) -- documented idempotent no-op.
            return Ok(());
        }
        // Synchronous, but always safe to call from any context --
        // including from within a task on this app's own runtime --
        // see `RuntimeOwner`'s doc comment. No need to route this
        // through `dispatch`/`spawn`: there is nothing left to run on
        // the runtime once it starts tearing down, and `shutdown_now`
        // itself never blocks regardless of whether it actually
        // reclaims the runtime this time around.
        self.inner.tokio_runtime.shutdown_now();
        Ok(())
    }

    /// Transitional handoff of this bootstrap's composed headless
    /// services to `crates/ui`'s not-yet-migrated `AppState`/`Services`
    /// construction. See [`LegacyComposition`]'s doc comment -- this is
    /// not part of the frontend-neutral operation surface a Flutter/Dart
    /// bridge would use.
    ///
    /// Gated on the same shutdown flag [`Self::dispatch`] checks: once
    /// [`Self::shutdown`] has been called, this returns a structured
    /// error instead of handing out a `Services` (and everything else
    /// `LegacyComposition` bundles) whose backing runtime may already be
    /// tearing down.
    pub fn take_legacy_composition(&self) -> Result<LegacyComposition, ApplicationError> {
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        Ok(self.inner.session.take_legacy_composition())
    }

    /// Starts opening and indexing an archive as a cancellable, event-
    /// broadcasting operation. Returns as soon as the operation is
    /// recorded `Accepted`; the archive listing, auto-password matching,
    /// and any interactive password challenge all happen on a task
    /// spawned through this app's own runtime handle (see
    /// `archive_ops::run_open_archive`) -- awaiting this method's future
    /// does not itself wait for the archive to finish opening. Subscribe
    /// via [`Self::subscribe_operations`] (or poll [`Self::operation`]) to
    /// observe `Started` / `Challenge` / `Completed { ArchiveOpened }` /
    /// `Failed`.
    pub async fn start_open_archive(
        &self,
        request: OpenArchiveRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) = inner.operations().begin(OperationKind::OpenArchive).await;
            // `tokio_handle()` returning `None` here would mean the
            // runtime finished tearing down in the instant between
            // `dispatch_async` obtaining its handle and this line --
            // see `AppRuntime::tokio_handle`'s doc comment for why that
            // is only a theoretical race, not a reachable one in a real
            // bootstrapped app. Handled defensively anyway: the
            // operation simply stays `Accepted` rather than panicking,
            // which is the least-bad outcome in an application that is
            // already on its way out.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(archive_ops::run_open_archive(
                    worker_inner,
                    operation_id,
                    cancel,
                    request,
                ));
            }
            operation_id
        })
        .await
    }

    /// Closes an open archive session. `NotFound` if `session_id` is
    /// unknown to this app instance -- including a structurally valid but
    /// never-issued (or already-closed) reconstructed id.
    pub async fn close_archive(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(
            move |inner| async move { inner.archive_sessions().close(session_id).await },
        )
        .await?
    }

    /// Lists one page of entries within `request.directory` of an open
    /// archive session, sorted/filtered/paginated per `request`. An
    /// immediate, in-memory query -- see `ArchiveSession::list_entries`.
    pub async fn list_entries(
        &self,
        session_id: ArchiveSessionId,
        request: ListEntriesRequest,
    ) -> Result<EntryPage, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let session = inner.archive_sessions().get(session_id).await?;
            Ok(session.list_entries(&request))
        })
        .await?
    }

    /// A point-in-time summary of an open archive session.
    pub async fn archive_snapshot(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<ArchiveSnapshot, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let session = inner.archive_sessions().get(session_id).await?;
            Ok(session.snapshot())
        })
        .await?
    }

    // ============= Task 6: extraction operation (start) =============
    // Kept in its own clearly-delimited section: a concurrent worktree
    // for a different task also edits this file, and this delimiter
    // minimizes merge friction between the two.

    /// Starts extracting entries (or, with an empty `entry_ids`, the
    /// whole archive) from an open session as a cancellable, event-
    /// broadcasting operation. Returns as soon as the operation is
    /// recorded `Accepted`; the facade owns process spawning and
    /// cancellation for the CLI extraction this drives -- egui no longer
    /// holds a child-process handle directly (see
    /// `crate::operations::extract`). Subscribe via
    /// [`Self::subscribe_operations`] to observe `Started` / `Progress` /
    /// `Challenge` (a password retry or a collision confirmation) /
    /// `Completed` / `Cancelled` / `Failed`.
    pub async fn start_extract(
        &self,
        request: crate::operations::extract::ExtractRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) = inner.operations().begin(OperationKind::Extract).await;
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(crate::operations::extract::run_extract(
                    worker_inner,
                    operation_id,
                    cancel,
                    request,
                ));
            }
            operation_id
        })
        .await
    }

    // ============== Task 6: extraction operation (end) ==============

    /// Subscribes to the operation-event stream. See
    /// `OperationRegistry::subscribe`.
    pub fn subscribe_operations(&self) -> tokio::sync::broadcast::Receiver<OperationEvent> {
        self.inner.operations().subscribe()
    }

    /// Answers a pending challenge on `operation_id` with `response`.
    /// Rejects (`Conflict`) a response whose id does not match the
    /// operation's actual pending challenge, or an operation with no
    /// pending challenge at all.
    pub async fn respond_to_challenge(
        &self,
        operation_id: OperationId,
        response: ChallengeResponse,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            inner
                .operations()
                .resolve_challenge(operation_id, response.id())
                .await?;
            inner.challenges().respond(operation_id, response)
        })
        .await?
    }

    /// Cooperatively cancels an in-flight operation. Idempotent -- see
    /// `OperationRegistry::cancel`.
    pub async fn cancel_operation(
        &self,
        operation_id: OperationId,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(
            move |inner| async move { inner.operations().cancel(operation_id).await },
        )
        .await?
    }

    /// A point-in-time snapshot of one operation's last-known state.
    pub async fn operation(
        &self,
        operation_id: OperationId,
    ) -> Result<OperationSnapshot, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            inner
                .operations()
                .operation(operation_id)
                .await
                .ok_or_else(|| {
                    ApplicationError::new(ApplicationErrorKind::NotFound, "no such operation")
                        .with_operation_id(operation_id)
                })
        })
        .await?
    }

    /// Up to `limit` operations, most-recently-created first.
    pub async fn recent_operations(
        &self,
        limit: u32,
    ) -> Result<Vec<OperationSnapshot>, ApplicationError> {
        self.dispatch_async(
            move |inner| async move { inner.operations().recent(limit as usize).await },
        )
        .await
    }

    // ---- Task 9: processing operations (Convert/Organize/Pipeline) ----
    // Everything in this section is this task's own addition; kept
    // together for the same reason as `AppRuntime`'s own Task 9 section
    // above. See `crate::operations::{convert, organize, pipeline}` for
    // each request type's own characterization/design notes, and
    // `runtime::processing_ops` for the shared execution loop and
    // background workers these three dispatch to.

    /// Starts converting a batch of archives to a target format as a
    /// cancellable, event-broadcasting operation. Rejects a structurally
    /// invalid request (empty `inputs`, an unrecognized `format`) before
    /// registering an operation at all -- see
    /// [`crate::operations::ConvertRequest::validate`].
    pub async fn start_convert(
        &self,
        request: ConvertRequest,
    ) -> Result<OperationId, ApplicationError> {
        let format = request.validate()?;
        self.dispatch_async(move |inner| async move {
            let (operation_id, _cancel) = inner.operations().begin(OperationKind::Convert).await;
            // `tokio_handle()` returning `None` here would mean the
            // runtime finished tearing down in the instant between
            // `dispatch_async` obtaining its handle and this line --
            // see `AppRuntime::tokio_handle`'s doc comment for why that
            // is only a theoretical race, not a reachable one in a real
            // bootstrapped app. Handled defensively anyway: the
            // operation simply stays `Accepted` rather than panicking,
            // which is the least-bad outcome in an application that is
            // already on its way out.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(processing_ops::run_convert(
                    worker_inner,
                    operation_id,
                    request,
                    format,
                ));
            }
            operation_id
        })
        .await
    }

    /// Starts organizing a batch of archives under one organization rule
    /// (layout) and one archive profile (output format/compression) as a
    /// cancellable, event-broadcasting operation. Rejects a structurally
    /// invalid request the same way [`Self::start_convert`] does (see
    /// [`crate::operations::OrganizeRequest::validate`]), and additionally
    /// confirms both ids name an existing rule/profile before registering
    /// an operation -- an I/O-requiring check `validate` itself cannot
    /// perform. See [`crate::operations::OrganizeRequest`]'s own doc
    /// comment for why this needs two ids and has no output transaction.
    pub async fn start_organize(
        &self,
        request: OrganizeRequest,
    ) -> Result<OperationId, ApplicationError> {
        let parsed_ids = request.validate()?;
        self.dispatch_async(move |inner| async move {
            processing_ops::resolve_rule_and_profile(
                &inner,
                parsed_ids.rule_id,
                parsed_ids.profile_id,
            )
            .await?;
            let (operation_id, _cancel) = inner.operations().begin(OperationKind::Organize).await;
            // See `start_convert`'s identical comment above -- the same
            // theoretical, non-reachable-in-practice race.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(processing_ops::run_organize(
                    worker_inner,
                    operation_id,
                    request,
                    parsed_ids,
                ));
            }
            Ok(operation_id)
        })
        .await?
    }

    /// Starts running either a saved preset or an ad-hoc step list over a
    /// batch of inputs as a cancellable, event-broadcasting operation.
    /// Rejects an empty `inputs` list (or a malformed ad-hoc step) the
    /// same way [`Self::start_convert`] does, and resolves a
    /// [`crate::operations::pipeline::PipelineSpecDto::Preset`] id against
    /// the saved presets file before registering an operation -- see
    /// [`crate::operations::PipelineRequest`]'s own doc comment.
    pub async fn start_pipeline(
        &self,
        request: PipelineRequest,
    ) -> Result<OperationId, ApplicationError> {
        request.validate()?;
        self.dispatch_async(move |inner| async move {
            let mut pipeline =
                processing_ops::resolve_pipeline_spec(&inner, &request.pipeline).await?;
            pipeline.output = request.destination.to_core();
            if let Some(policy) = request.collision_policy {
                pipeline.collision_policy = Some(policy.to_core());
            }
            let (operation_id, _cancel) = inner.operations().begin(OperationKind::Pipeline).await;
            // See `start_convert`'s identical comment above -- the same
            // theoretical, non-reachable-in-practice race.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(processing_ops::run_pipeline(
                    worker_inner,
                    operation_id,
                    pipeline,
                    request.inputs,
                ));
            }
            Ok(operation_id)
        })
        .await?
    }

    /// Runs `work` against the composed session state on this app's own
    /// Tokio runtime, then awaits the result -- so the computation
    /// itself is never at the mercy of whatever executor happens to be
    /// polling the returned future. `work` runs synchronously once
    /// dispatched (today's `capabilities`/`health` need no I/O), but
    /// every facade method funnels through this one dispatch point so
    /// future methods that *do* need to await internal work inherit the
    /// same executor-agnostic behavior for free.
    async fn dispatch<T, F>(&self, work: F) -> Result<T, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(&AppRuntime) -> T + Send + 'static,
    {
        // The authoritative "has shutdown() been called" signal. Unlike
        // an earlier version of this method, this is *not* redundant
        // with checking `RuntimeOwner::handle()` for `None`: since
        // `shutdown_now` puts its `Arc` back whenever something else
        // (in practice, always -- see `RuntimeOwner`'s and
        // `ArclainApp::shutdown`'s doc comments) is still using the
        // runtime, `handle()` keeps returning `Some` in the overwhelming
        // common case even after `shutdown()` has run.
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        // Reached only in the narrow case where the runtime *did*
        // finish a real teardown (every other reference, including
        // `session`'s, was already gone) between the flag check above
        // and here -- `shutdown()` always sets the flag before calling
        // `shutdown_now`, so this should never observe `None` while the
        // flag check above observed `false`, but handling it defensively
        // costs nothing and avoids ever spawning onto a torn-down
        // runtime.
        let Some(handle) = self.inner.tokio_runtime.handle() else {
            return Err(shutdown_error());
        };
        let inner = self.inner.clone();
        handle
            .spawn(async move { work(&inner) })
            .await
            .map_err(|join_error| {
                ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                    .with_diagnostic(join_error.to_string())
            })
    }

    /// Generalizes [`Self::dispatch`] to asynchronous work: `work` gets an
    /// owned `Arc<AppRuntime>` (rather than a borrow) precisely so it can
    /// return a future that outlives the borrow -- await internal locks,
    /// query the archive-session store, and so on -- while still running
    /// entirely on this app's own runtime via the same `spawn`-then-await
    /// pattern `dispatch` uses. Every facade method that awaits anything
    /// beyond trivial synchronous computation goes through this rather
    /// than through `dispatch`. Checks the same `shut_down` flag `dispatch`
    /// does, for the same reason -- see `dispatch`'s own comments for why
    /// the flag, not `RuntimeOwner::handle()` returning `None`, is the
    /// authoritative signal.
    async fn dispatch_async<T, F, Fut>(&self, work: F) -> Result<T, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<AppRuntime>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = T> + Send + 'static,
    {
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        let Some(handle) = self.inner.tokio_handle() else {
            return Err(shutdown_error());
        };
        let inner = self.inner.clone();
        handle.spawn(work(inner)).await.map_err(|join_error| {
            ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                .with_diagnostic(join_error.to_string())
        })
    }
}

fn shutdown_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "application has been shut down",
    )
    .with_diagnostic("ArclainApp::shutdown was already called; no further facade calls are served")
    .with_recoverability(Recoverability::Fatal)
}

#[cfg(test)]
mod runtime_owner_tests {
    use super::*;

    /// Deterministic, surgical reproduction of the bug `RuntimeOwner`
    /// exists to fix: dropping the *last* reference to the runtime from
    /// inside a task running on that very runtime must not panic.
    ///
    /// Before the fix (a bare `Arc<tokio::runtime::Runtime>` field with
    /// no custom `Drop`), the equivalent of the `drop(owner)` below
    /// reached `tokio::runtime::Runtime`'s ordinary blocking `Drop` from
    /// inside one of its own worker threads and panicked ("Cannot drop
    /// a runtime in a context where blocking is not allowed"). This is
    /// exactly the scenario `ArclainApp::dispatch` can create in
    /// practice: it moves a clone of `Arc<AppRuntime>` (which contains
    /// this same `RuntimeOwner`) into a task spawned on the app's own
    /// runtime; if that task's clone ends up being the last one alive
    /// when it finishes, its drop runs on a worker thread.
    ///
    /// Uses the runtime's own `Handle::block_on` (from a plain, non-async
    /// `#[test]` fn -- no ambient runtime) to wait for the spawned task
    /// and assert it did not panic, which a bare `.await` inside an
    /// `async` test could not do as reliably: a panic inside a spawned
    /// task is caught by Tokio and surfaced through the `JoinHandle`,
    /// not by crashing the process, so the assertion must actually
    /// inspect that `JoinHandle`'s result.
    #[test]
    fn dropping_the_last_reference_from_within_its_own_worker_thread_does_not_panic() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        );
        let handle = runtime.handle().clone();
        let owner = RuntimeOwner::new(runtime.clone());

        let join_handle = handle.spawn(async move {
            drop(owner);
        });

        // Drop our own `Arc<Runtime>` clone now, so the spawned task's
        // `owner` (holding the *other* clone) is the one left holding
        // the last reference when it runs -- exactly the ordering that
        // reproduces the bug.
        drop(runtime);

        handle.block_on(join_handle).expect(
            "dropping RuntimeOwner's last reference on its own worker thread must not panic",
        );
    }

    /// The other half of `RuntimeOwner`'s contract: if some other clone
    /// of the underlying `Arc<Runtime>` is still alive elsewhere (for
    /// example, `arclain_core::services::Services::tokio_runtime`,
    /// handed out through `ArclainApp::take_legacy_composition` and
    /// possibly still held by `crates/ui` long after this `RuntimeOwner`
    /// is gone), dropping our own clone must be a plain, always-safe
    /// reference-count decrement -- never an attempt to shut down a
    /// runtime something else is still using.
    #[test]
    fn dropping_one_of_several_clones_is_a_plain_refcount_decrement() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap(),
        );
        let still_alive_elsewhere = runtime.clone();
        let owner = RuntimeOwner::new(runtime);

        // `owner` is not the last reference (`still_alive_elsewhere`
        // keeps the runtime running) -- dropping it must not shut
        // anything down.
        drop(owner);

        // The runtime is still fully usable through the surviving clone.
        let result = still_alive_elsewhere.block_on(async { 1 + 1 });
        assert_eq!(
            result, 2,
            "runtime must still be usable while another clone is alive"
        );
    }
}
