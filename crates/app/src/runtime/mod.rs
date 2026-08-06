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

/// `pub(crate)` rather than private only so `crate::operations::merge`
/// can reuse [`archive_ops::is_password_error`]: a merge reads back the
/// very same `SevenZipCli::run_status` error text an archive open does,
/// so it classifies a password failure with the same predicate instead of
/// keeping a fourth copy of that string list.
pub(crate) mod archive_ops;
mod bootstrap;
mod drag_stage_ops;
mod layout_ops;
mod organization_ops;
mod paths;
mod process_ops;
mod processing_ops;
mod session_store;
mod settings_ops;

pub use bootstrap::{BootstrapConfig, BootstrapOverrides};
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
use crate::challenge::{ChallengeResponse, SecretInput};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{
    OperationEvent, OperationKind, OperationResult, OperationSnapshot, OperationState, SessionEvent,
};
use crate::ids::{ArchiveSessionId, MaterializationLeaseId, OperationId, PluginSessionId};
use crate::materialization::{MaterializationLease, MaterializationStore, MaterializeRequest};
use crate::operations::{
    ArchiveMutationRequest, ChallengeWaiters, ConvertRequest, OperationRegistry, OrganizeRequest,
    PipelineRequest,
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
/// and a compatibility caller can clone that service bundle through
/// `ArclainApp::take_legacy_composition`, potentially outliving this
/// `RuntimeOwner`. `Drop` (and [`Self::shutdown_now`]) account for this:
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
/// always safe, being a plain decrement. If a compatibility caller takes
/// a `Services` clone and lets it outlive every `ArclainApp` clone, its
/// *own*, later drop is outside this wrapper's reach entirely -- a
/// pre-existing limitation of `arclain_core::services::Services` holding
/// an unwrapped `Arc<Runtime>`, not something introduced or fixable here.
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
    /// How many stored password rules `bootstrap` rewrote on the way up
    /// -- see [`ArclainApp::startup_password_rule_upgrades`].
    startup_password_rule_upgrades: usize,
    session: SessionStore,
    /// The cancellable, event-broadcasting registry every asynchronous
    /// application operation (starting with `start_open_archive`) is
    /// tracked through.
    operations: OperationRegistry,
    /// Every open archive session, keyed by the `ArchiveSessionId` this
    /// store itself mints. `Arc`-wrapped (unlike most other `AppRuntime`
    /// fields, which are already reachable through `AppRuntime`'s own
    /// `Arc`) so `crate::plugins::ArchiveContextBridge` -- constructed
    /// once and installed on `PluginManager`, entirely independent of
    /// any one `ArclainApp` clone's lifetime -- can hold its own cheap
    /// clone of the exact same store instance rather than a second,
    /// disconnected one.
    archive_sessions: Arc<ArchiveSessionStore>,
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
    /// Every live materialization lease -- see `crate::materialization`.
    materialization: MaterializationStore,
    /// A handle to abort the materialization cleanup task
    /// (`crate::materialization::run_cleanup_task`), set once shortly
    /// after that task is spawned in [`ArclainApp::bootstrap`] (it cannot
    /// be set any earlier: the task needs a `Weak<AppRuntime>`, which does
    /// not exist until this struct is already wrapped in an `Arc` -- see
    /// that method's own comment). `Mutex<Option<..>>` rather than a plain
    /// field for exactly that reason: briefly `None` between this
    /// struct's own construction and the moment `bootstrap` finishes
    /// spawning the task. Aborting is a *prompt* stop -- [`ArclainApp::shutdown`]
    /// calls it explicitly rather than relying solely on the task's own
    /// `Weak::upgrade` check, which would otherwise only notice this
    /// application is gone on its next timer tick (up to a full
    /// `materialization_cleanup_interval_override`/`DEFAULT_CLEANUP_INTERVAL`
    /// later), or not at all if some other reference (see `RuntimeOwner`'s
    /// own doc comment on `SessionStore::core_services`) keeps `AppRuntime`
    /// itself alive past `shutdown()`.
    cleanup_task_handle: parking_lot::Mutex<Option<tokio::task::AbortHandle>>,
    /// Serializes every settings/secrets/vault *mutation* (`update_settings`,
    /// `set_gameta_api_key`, `set_socks5_password`, `move_vault`,
    /// `rekey_vault`, `upsert_password_rule`, `delete_password_rule`) end
    /// to end -- see `runtime::settings_ops`'s own module doc comment for
    /// why this, rather than just `SessionStore::mutable`'s fast
    /// `RwLock`, is what makes `update_settings`'s optimistic-revision
    /// check actually race-free. Read-only settings methods never take
    /// this.
    settings_write_lock: tokio::sync::Mutex<()>,
    /// Serializes destructive cache-maintenance requests. These run on
    /// the blocking pool, so a synchronous mutex is the appropriate
    /// guard and is never held across an `.await`.
    cache_maintenance_lock: parking_lot::Mutex<()>,
    /// Set once by [`ArclainApp::shutdown`]. Every [`ArclainApp::dispatch`]
    /// call checks this first so a clone that outlives shutdown (held by
    /// another part of the program, or racing a concurrent shutdown call)
    /// gets a structured error instead of silently spawning onto a runtime
    /// that may already be tearing down. Also gates
    /// [`ArclainApp::take_legacy_composition`], so a post-shutdown caller
    /// cannot obtain a live `Services` either.
    shut_down: AtomicBool,

    // ==================== Task 11: plugin sessions (start) ====================
    // Kept in its own clearly-delimited section for the same reason as the
    // Task 9 sections elsewhere in this file: a concurrent worktree may be
    // touching this same shared file for an unrelated task.
    /// Every open renderer-neutral plugin session.
    plugin_sessions: crate::plugins::PluginSessionStore,
    /// Which archive session the frontend last reported as active, via
    /// [`ArclainApp::set_active_archive_session`].
    active_archive_session: crate::plugins::ActiveArchiveSession,
    // ==================== Task 11: plugin sessions (end) ====================
    tokio_runtime: RuntimeOwner,
}

impl AppRuntime {
    pub(crate) fn operations(&self) -> &OperationRegistry {
        &self.operations
    }

    pub(crate) fn archive_sessions(&self) -> &ArchiveSessionStore {
        &self.archive_sessions
    }

    /// A cloned handle (cheap: an `Arc` bump) to the same store
    /// [`Self::archive_sessions`] borrows from -- for constructing a
    /// long-lived, independently-owned adapter such as
    /// `crate::plugins::ArchiveContextBridge`, which cannot borrow
    /// `&AppRuntime` for its own lifetime.
    pub(crate) fn archive_sessions_handle(&self) -> Arc<ArchiveSessionStore> {
        self.archive_sessions.clone()
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

    /// The 7-Zip CLI fallback backend bootstrap detected -- the one
    /// backend that can stream-and-hash *decrypted* entry content for
    /// any archive format, which is why the encrypted-CRC backfill
    /// computes through it rather than a format's primary backend
    /// (whose `crc32_of_entry` may return the stored header value:
    /// zeroed for AES zip entries). `None` when bootstrap found no
    /// 7-Zip; callers degrade rather than fail, since nothing that
    /// merely *browses* an archive needs it. Cheap: `SevenZipCli` clones
    /// its resolved executable path.
    pub(crate) fn fallback_backend(
        &self,
    ) -> Option<arclain_core::backends::sevenz_cli::SevenZipCli> {
        self.session.fallback_backend.clone()
    }

    pub(crate) fn extract_runner(&self) -> Arc<dyn crate::operations::extract::ExtractRunner> {
        self.extract_runner.clone()
    }

    pub(crate) fn materialization(&self) -> &MaterializationStore {
        &self.materialization
    }

    pub(crate) fn pass_rules(&self) -> Vec<PassRule> {
        self.session.mutable.read().pass_rules.clone()
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

    // ==================== Task 11: plugin sessions (start) ====================

    pub(crate) fn plugin_manager(
        &self,
    ) -> Option<Arc<parking_lot::Mutex<arclain_plugins::PluginManager>>> {
        self.session.plugin_manager.clone()
    }

    pub(crate) fn content_cache(&self) -> Option<&Arc<arclain_core::ContentCache>> {
        self.session.content_cache.as_ref()
    }

    pub(crate) fn plugin_sessions(&self) -> &crate::plugins::PluginSessionStore {
        &self.plugin_sessions
    }

    pub(crate) fn active_archive_session(&self) -> &crate::plugins::ActiveArchiveSession {
        &self.active_archive_session
    }

    // ==================== Task 11: plugin sessions (end) ====================

    // ============ Task 12: host-owned display images (start) ================

    /// Backs [`ArclainApp::materialized_resource_limit`].
    ///
    /// Resolved once, at bootstrap, from the resource configuration this
    /// instance composed: `ResourceManager` exposes it as a plain read of
    /// its own immutable `ResourceConfig`, and changing it needs `&mut`,
    /// which nothing holding the shared `Arc` can obtain. The fallback is
    /// the same default `ResourceManager` itself would report, so an
    /// instance composed without one answers identically instead of
    /// forcing every caller to invent a bound of its own.
    pub(crate) fn materialized_resource_limit(&self) -> usize {
        self.session
            .resource_manager
            .as_ref()
            .map_or(arclain_data::DEFAULT_MAX_RESOURCE_SIZE_BYTES, |manager| {
                manager.materialization_limit()
            })
    }

    // ============= Task 12: host-owned display images (end) =================
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
        Self::bootstrap_with_overrides(config, BootstrapOverrides::default())
    }

    /// Bootstraps with process-local application overrides. This is the
    /// fixture-safe counterpart to [`Self::bootstrap`]: frontends can
    /// provide external-tool paths without reaching into configuration
    /// databases owned by this crate.
    pub fn bootstrap_with_overrides(
        config: BootstrapConfig,
        overrides: BootstrapOverrides,
    ) -> Result<Self, ApplicationError> {
        // Read before `config` moves into `bootstrap::run` -- the resolved
        // interval is only needed here, to spawn the materialization
        // cleanup task below, once `inner` actually exists to clone into
        // it (see `crate::materialization::run_cleanup_task`'s own comment
        // on why it takes a `Weak`, which likewise cannot be produced
        // until this constructor wraps `bootstrap::run`'s return value in
        // an `Arc`).
        let cleanup_interval = config
            .materialization_cleanup_interval_override
            .unwrap_or(crate::materialization::DEFAULT_CLEANUP_INTERVAL);
        let runtime = bootstrap::run(config, overrides)?;
        let inner = Arc::new(runtime);
        if let Some(handle) = inner.tokio_handle() {
            // `Weak`, not `Arc`: see `run_cleanup_task`'s own doc comment
            // for why holding a strong reference here would keep
            // `AppRuntime` permanently alive. The join handle's own
            // `AbortHandle` is retained separately so `shutdown()` can stop
            // this task promptly and explicitly, rather than only via the
            // task noticing `upgrade()` fail on its own next timer tick.
            let cleanup_inner = Arc::downgrade(&inner);
            let join_handle = handle.spawn(crate::materialization::run_cleanup_task(
                cleanup_inner,
                cleanup_interval,
            ));
            *inner.cleanup_task_handle.lock() = Some(join_handle.abort_handle());
        }
        Ok(Self { inner })
    }

    /// The directories this instance resolved at bootstrap.
    pub fn paths(&self) -> &AppPaths {
        &self.inner.paths
    }

    /// How many stored password rules this bootstrap broadened as it
    /// started, out of the rules it found.
    ///
    /// Rules auto-saved before the pattern heuristic existed match
    /// exactly one archive, so siblings sharing a product code or maker
    /// bracket re-prompt for a password they already have. Bootstrap
    /// rewrites only those provably-narrow rules, before anything can
    /// read a rule, and this reports how many it touched -- purely so a
    /// frontend can tell the user something changed under them. It is a
    /// count of a completed migration, not a pending one: a frontend
    /// that ignores it loses a notification, never a correction.
    ///
    /// `0` on every launch after the first, since a broadened rule no
    /// longer matches the narrow fingerprint.
    pub fn startup_password_rule_upgrades(&self) -> usize {
        self.inner.startup_password_rule_upgrades
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
        // Stops the materialization cleanup task promptly and explicitly,
        // rather than leaving it to notice on its own (see
        // `AppRuntime::cleanup_task_handle`'s own doc comment): it would
        // otherwise keep calling `sweep_expired` right up until its next
        // `Weak::upgrade` fails, which this method's own remaining steps
        // do not wait for, and might never even reach if some other
        // reference keeps `AppRuntime` alive past this call (see
        // `RuntimeOwner`'s doc comment). `None` only in the narrow case
        // `bootstrap` itself already treats defensively (no runtime handle
        // was available to spawn the task onto in the first place).
        if let Some(handle) = self.inner.cleanup_task_handle.lock().take() {
            handle.abort();
        }
        // Removes every outstanding materialization lease's directory
        // before the runtime starts tearing down. Plain synchronous work
        // (the store's own internal locking is `parking_lot`, not
        // `tokio::sync`, precisely so this never needs `dispatch`/`spawn`
        // either) -- safe to call directly here, same as `shutdown_now`
        // below.
        self.inner.materialization().clear_all();
        // The last chance to take what a plugin wrote about itself. Must
        // precede `shutdown_now` below: it needs the runtime's blocking
        // pool for the database round trip. See
        // `settings_ops::run_flush_all_plugin_settings` for which guest
        // entries this is the *only* pull for -- an installed plugin's
        // `init`, a top-tab query, and the event worker that runs guests
        // with no plugin session open at all.
        settings_ops::run_flush_all_plugin_settings(&self.inner).await;
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

    /// Transitional handoff of this bootstrap's legacy state to
    /// `crates/ui`'s not-yet-migrated `AppState` construction. See
    /// [`LegacyComposition`]'s doc comment -- this is not part of the
    /// frontend-neutral operation surface a Flutter/Dart bridge would use.
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

    /// Every entry of an open archive session -- files and directories,
    /// at every depth -- as one [`crate::archive::ArchiveInventory`], in
    /// depth-first tree order (see that type's own doc comment).
    ///
    /// The whole-archive counterpart to [`Self::list_entries`], for the
    /// consumers that genuinely need the full tree at once: a
    /// folder-tree panel's directory set (directory rows carry the
    /// `EntryKind::Directory` flag [`Self::archive_file_paths`]'s
    /// files-only list cannot), whole-archive aggregate totals, a
    /// drag-out's recursive folder expansion, and a plugin event's entry
    /// snapshot. A consumer that needs one directory, a count, or a
    /// window of paths wants [`Self::list_entries`],
    /// `ArchiveSnapshot::entry_count`, or [`Self::archive_file_paths`]
    /// instead.
    ///
    /// `O(entries)`: materializes and clones the whole tree, the same
    /// cost class [`Self::archive_file_paths`] documents for its own
    /// full-list read. Callers should fetch once per
    /// `(session, revision)` and cache, not call per frame or per
    /// keystroke. `NotFound` for an unknown or already-closed session id.
    pub async fn list_all_entries(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<crate::archive::ArchiveInventory, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let session = inner.archive_sessions().get(session_id).await?;
            Ok(session.inventory())
        })
        .await?
    }

    /// What a plugin has reported about the *product* the archive open
    /// in `session_id` contains, in the vocabulary a frontend displays
    /// -- see [`crate::archive::ProductMetadataSummary`].
    ///
    /// `Ok(None)` when no plugin has reported anything for this session
    /// yet, and also when what it reported does not parse: a display
    /// surface has nothing different to do in those two cases, and a
    /// metadata-less plan is what the planner already makes of an
    /// unparseable document. `NotFound` for an unknown or already-closed
    /// session id.
    ///
    /// The read counterpart of [`Self::archive_snapshot`]'s raw
    /// `metadata` document, and preferred over it for anything that
    /// reads *fields*: the document's parse rule stays inside this
    /// application (shared with the organize planner, so a panel cannot
    /// display a title the plan was not built from), and the payloads a
    /// display surface never needs -- the document itself, inline
    /// screenshot bytes -- do not cross the boundary at all.
    ///
    /// There is deliberately no write counterpart. Metadata is written
    /// by plugins, through the `emit_metadata` host function and
    /// [`crate::plugins::ArchiveContextBridge`]; every announcement of
    /// such a write reaches a frontend as
    /// [`crate::event::SessionEvent::MetadataChanged`]. No frontend path
    /// writes metadata, and adding a second writer here would give the
    /// planner two sources to disagree about.
    pub async fn product_metadata(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<Option<crate::archive::ProductMetadataSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let session = inner.archive_sessions().get(session_id).await?;
            Ok(crate::archive::product_metadata_from_document(
                session.metadata(),
            ))
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

    /// Reads one entry's decoded text content out of an open session --
    /// what a file-edit dialog populates its editor from.
    ///
    /// The session supplies the backend and the password it was opened
    /// with, so a caller never handles the archive's secret at all. This
    /// replaces the last frontend-side raw archive read, which selected
    /// its own backend and passed a password the frontend had been
    /// stamped with from the session's own handle -- and, with it, the
    /// transitional `session_archive_handle` that existed to supply that
    /// stamp.
    ///
    /// Reads decoded content through the backend on a blocking thread;
    /// long enough for a large entry that a caller must not await it on
    /// a render path. `NotFound` for an unknown or already-closed
    /// session, and for an `entry_id` this session's current index never
    /// minted; `InvalidInput` for a directory; `PasswordRequired` when
    /// the backend failure is password-shaped.
    pub async fn read_entry_text(
        &self,
        session_id: ArchiveSessionId,
        entry_id: crate::ids::EntryId,
    ) -> Result<String, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            archive_ops::run_read_entry_text(inner, session_id, entry_id).await
        })
        .await?
    }

    /// Computes the CRC-32 of every encrypted file entry whose listing
    /// carried none, writes the results into the session's own index
    /// (bumping its revision so cached pages/inventories refetch), and
    /// reports what happened -- see
    /// [`crate::archive::EncryptedCrcBackfill`] for the answer's fields
    /// and `runtime::archive_ops::run_backfill_encrypted_crcs` for the
    /// password ladder and the deliberate carried-over behaviors.
    ///
    /// Mechanism, not policy: whether to call this at all (the
    /// `encrypted_crc_policy` setting -- `on_access` skips it entirely,
    /// `prompt_on_open` follows a `password_available: false` answer
    /// with a prompt) stays the caller's decision, exactly where the
    /// pre-facade UI kept it.
    ///
    /// Reads and hashes every targeted entry's *decrypted content*
    /// through the CLI backend -- for a large archive of encrypted files
    /// this can run for a long time, so call it from a background task,
    /// never a render path. Per-entry failures (wrong password included)
    /// are skipped, surfacing as `computed: 0`. A mutation that lands
    /// mid-computation invalidates the whole batch rather than applying
    /// stale sums. `NotFound` for an unknown or already-closed session.
    pub async fn backfill_encrypted_crcs(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<crate::archive::EncryptedCrcBackfill, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            archive_ops::run_backfill_encrypted_crcs(inner, session_id).await
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

    // ========= Task 8: archive mutation operation (start) ==========

    /// Starts an archive-mutating operation (`AddFiles`/`DeleteEntries`/
    /// `ReplaceText`) as a cancellable, event-broadcasting operation.
    /// Returns as soon as the operation is recorded `Accepted`; the
    /// backend call, capability gating, revision check, and post-mutation
    /// reindex all happen on a task spawned through this app's own
    /// runtime handle (see `crate::operations::archive_mutation`).
    /// Subscribe via [`Self::subscribe_operations`] to observe `Started` /
    /// `SnapshotChanged` (on success, exactly once, immediately before
    /// `Completed`) / `Completed` / `Cancelled` / `Failed`.
    pub async fn start_archive_mutation(
        &self,
        request: ArchiveMutationRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) =
                inner.operations().begin(OperationKind::ArchiveModify).await;
            // See `start_extract`'s identical comment above -- the same
            // theoretical, non-reachable-in-practice race.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(crate::operations::archive_mutation::run_archive_mutation(
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

    // ========== Task 8: archive mutation operation (end) ===========

    // ============ Task c9: multi-part archive merge (start) ===========
    // Kept in its own clearly-delimited section for the same reason as
    // every other section in this file: concurrent worktrees edit it too.

    /// Merges a split multi-part archive set into one archive, as a
    /// cancellable, event-broadcasting operation. Returns as soon as the
    /// operation is recorded `Accepted`; re-confirming the set on disk,
    /// extraction, recompression, and any password-challenge retry all
    /// happen on tasks spawned through this app's own runtime handle (see
    /// [`crate::operations::merge`]). Subscribe via
    /// [`Self::subscribe_operations`] to observe `Started` / `Progress`
    /// (percent out of 100, with core's own phase message) / `Challenge`
    /// (a password retry for an encrypted set) / `Completed { Merged }` /
    /// `Cancelled` / `Failed`.
    ///
    /// The set itself comes from [`crate::archive::detect_multipart`], a
    /// free function rather than a method here -- see its own module doc
    /// comment for why.
    ///
    /// Read [`crate::operations::merge::run_merge`]'s doc comment before
    /// treating `Cancelled` as "nothing was written": cancellation is
    /// checkpoint-based, and one of core's checkpoints sits *after* the
    /// output archive is complete.
    pub async fn start_merge(
        &self,
        request: crate::operations::MergeRequest,
    ) -> Result<OperationId, ApplicationError> {
        // Structural validation before the operation is registered, so a
        // malformed request never leaves a phantom `OperationId` behind
        // -- exactly `start_convert`/`start_organize`'s own ordering.
        request.validate()?;
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) = inner.operations().begin(OperationKind::Merge).await;
            // See `start_extract`'s identical comment above -- the same
            // theoretical, non-reachable-in-practice race.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(crate::operations::merge::run_merge(
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

    // ============= Task c9: multi-part archive merge (end) ============

    // ============= Task 7: materialization leases (start) =============

    /// Starts materializing one archive entry onto a real local disk path
    /// as a cancellable, event-broadcasting operation. Returns as soon as
    /// the operation is recorded `Accepted`; the actual extraction (and
    /// any password-challenge retry) happens on a task spawned through
    /// this app's own runtime handle (see `crate::materialization::run_materialize`).
    /// Subscribe via [`Self::subscribe_operations`] to observe `Started` /
    /// `Challenge` / `Completed { Materialized }` / `Cancelled` / `Failed`.
    pub async fn start_materialization(
        &self,
        request: MaterializeRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) = inner.operations().begin(OperationKind::Materialize).await;
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(crate::materialization::run_materialize(
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

    /// Extends `id`'s expiry by its configured TTL from now, returning the
    /// new `expires_at_unix_ms`. `NotFound` if `id` is unknown, already
    /// released, or already expired -- a caller whose renewal lost the
    /// race against expiry must re-materialize rather than assume the
    /// lease is still valid.
    pub async fn renew_materialization(
        &self,
        id: MaterializationLeaseId,
    ) -> Result<i64, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            inner
                .materialization()
                .renew(id, crate::materialization::current_unix_ms())
        })
        .await?
    }

    /// Releases a materialization lease, removing its owned directory.
    /// Idempotent -- see `MaterializationStore::release`.
    pub async fn release_materialization(
        &self,
        id: MaterializationLeaseId,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let Some(handle) = inner.tokio_handle() else {
                return Ok(());
            };
            handle
                .spawn_blocking(move || inner.materialization().release(id))
                .await
                .map_err(|join_error| {
                    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                        .with_diagnostic(join_error.to_string())
                })?
        })
        .await?
    }

    /// A point-in-time read of one materialization lease.
    pub async fn materialization(
        &self,
        id: MaterializationLeaseId,
    ) -> Result<MaterializationLease, ApplicationError> {
        self.dispatch_async(move |inner| async move { inner.materialization().get(id) })
            .await?
    }

    /// Reads up to `length` bytes (bounded by
    /// `crate::materialization::MAX_MATERIALIZATION_READ_BYTES`) starting
    /// at `offset` from a materialized lease's own file. `NotFound` on a
    /// released/expired/unknown lease; an offset at or past end-of-file
    /// yields an empty result rather than an error. Runs the actual file
    /// read through `spawn_blocking` (unlike `renew`/`release`/`materialization`,
    /// which are plain in-memory lookups): a bounded read is still real,
    /// potentially slow disk I/O, not the "trivial computation" this
    /// crate's `dispatch`/`dispatch_async` doc comments reserve for
    /// running directly.
    pub async fn read_materialization_range(
        &self,
        id: MaterializationLeaseId,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let Some(handle) = inner.tokio_handle() else {
                return Err(ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "application has been shut down",
                )
                .with_recoverability(Recoverability::Fatal));
            };
            handle
                .spawn_blocking(move || inner.materialization().read_range(id, offset, length))
                .await
                .map_err(|join_error| {
                    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                        .with_diagnostic(join_error.to_string())
                })?
        })
        .await?
    }

    // ============== Task 7: materialization leases (end) ==============

    /// Subscribes to the operation-event stream. See
    /// `OperationRegistry::subscribe`.
    pub fn subscribe_operations(&self) -> tokio::sync::broadcast::Receiver<OperationEvent> {
        self.inner.operations().subscribe()
    }

    /// Subscribes to the session-event stream: changes to an archive
    /// session's state that happen outside any operation (a plugin
    /// writing metadata, or renaming the archive). See
    /// `crate::archive::ArchiveSessionStore::subscribe_session_events`
    /// and [`crate::event::SessionEvent`]'s own doc comment for the
    /// delivery contract.
    pub fn subscribe_session_events(&self) -> tokio::sync::broadcast::Receiver<SessionEvent> {
        self.inner.archive_sessions().subscribe_session_events()
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

    /// Starts organizing archives under one organization rule (layout)
    /// and one archive profile (output format/compression) as a
    /// cancellable, event-broadcasting operation. Rejects a structurally
    /// invalid request the same way [`Self::start_convert`] does (see
    /// [`crate::operations::OrganizeRequest::validate`]), and additionally
    /// confirms both ids name an existing rule/profile -- and, for a
    /// session-bound request, that the session is still open -- before
    /// registering an operation, all I/O-requiring checks `validate`
    /// itself cannot perform. See [`crate::operations::OrganizeRequest`]'s
    /// own doc comment for why this needs two ids and has no output
    /// transaction.
    ///
    /// # Applying what was previewed
    ///
    /// Setting [`crate::operations::OrganizeRequest::archive_session_id`]
    /// organizes the archive open in that session, from the metadata that
    /// session holds -- the exact plan [`Self::preview_organize_plan`]
    /// reports for the same session and rule. Without it, this resolves
    /// metadata per input from the DLsite library instead, which is the
    /// only thing a path-only batch can do and is *not* what a preview
    /// shows. A frontend that previewed a plan must bind the session it
    /// previewed, or it applies a different plan than the one the user
    /// approved.
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
            let binding = match request.archive_session_id {
                Some(session_id) => {
                    Some(processing_ops::resolve_session_binding(&inner, session_id).await?)
                }
                None => None,
            };
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
                    binding,
                ));
            }
            Ok(operation_id)
        })
        .await?
    }

    /// Starts running either a saved preset or an ad-hoc step list over a
    /// batch of inputs as a cancellable, event-broadcasting operation.
    /// Rejects an empty file list (or a malformed ad-hoc step) the
    /// same way [`Self::start_convert`] does, and resolves a
    /// [`crate::operations::pipeline::PipelineSpecDto::Preset`] id against
    /// the saved presets file before registering an operation -- see
    /// [`crate::operations::PipelineRequest`]'s own doc comment.
    ///
    /// A [`crate::operations::pipeline::PipelineInputsDto::Folder`] is
    /// expanded once the operation has started, by `arclain_core`'s own
    /// definition of an archive in a directory -- so "process this
    /// folder" means the folder as it is when the run begins, not as it
    /// was when the request was built. A folder that cannot be read
    /// fails the operation; one that holds no archive completes having
    /// processed nothing, which is what `arclain_core` does with the
    /// same folder and what [`Self::preview_pipeline`] warns about
    /// beforehand.
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

    // ============= Task 10: settings, secrets, vault (start) ============
    // Kept in its own clearly-delimited section for the same reason as
    // Task 9's own section above: a concurrent worktree (Task 11,
    // plugins) also edits this file. Every method here is a thin
    // dispatch wrapper; the actual logic lives in
    // `crate::settings` (pure DTOs/validation) and
    // `runtime::settings_ops` (the `AppRuntime`-touching execution
    // layer) -- see both modules' own doc comments.
    //
    // The plugins section immediately below this one is Task 11's own
    // equally delimited block, kept separate for the same reason.

    /// A full, point-in-time view of every non-secret application
    /// setting.
    ///
    /// A vault that never opened is **not** a failure: it is reported as
    /// `security.vault_available: false`, the two secret-`configured`
    /// flags read `false`, and the archive/network/general settings
    /// reflect whatever `arclain_core::UserConfig` this instance loaded
    /// regardless.
    ///
    /// It does fail in two cases, and callers must treat both as real:
    /// after [`Self::shutdown`], and when a vault that *did* open then
    /// errors on the two secret-presence reads this assembles the
    /// snapshot from. Both mean the application can no longer report
    /// its own configuration — so a caller must not substitute
    /// placeholder settings and carry on. `crates/ui`'s startup depends
    /// on exactly that: acting on the placeholder its settings mirrors
    /// hold before their first read would delete the user's saved tab
    /// session (see `AppState::refresh_settings_signals`'s own note),
    /// so it propagates this error rather than logging it.
    pub async fn settings(&self) -> Result<crate::settings::SettingsSnapshot, ApplicationError> {
        self.dispatch_async(|inner| async move { settings_ops::run_settings(&inner).await })
            .await?
    }

    /// Performs one application-owned cache maintenance task on the
    /// blocking pool. No task accepts a filesystem path: the application
    /// resolves its live cache/database handles itself, and clear-content
    /// is scoped to cache blobs/partials so sibling resource and
    /// materialization directories remain intact.
    pub async fn maintain_cache(
        &self,
        task: crate::settings::CacheMaintenanceTask,
    ) -> Result<crate::settings::CacheMaintenanceReport, ApplicationError> {
        self.dispatch_blocking(move |inner| settings_ops::run_cache_maintenance(inner, task))
            .await?
    }

    /// Reports the configured gameta integration's already-known startup
    /// state without performing another network request.
    ///
    /// The version, when available, is the one cached by the health check
    /// that ran while the application composed its client. This gives a
    /// frontend exactly the status it needs for initial rendering without
    /// exposing `arclain_network::GametaClient` or any other service handle.
    pub async fn gameta_connection_status(
        &self,
    ) -> Result<crate::settings::GametaConnectionStatusDto, ApplicationError> {
        self.dispatch(|inner| {
            let enabled = inner
                .session
                .mutable
                .read()
                .user_config
                .gameta_server_enabled;
            let client = inner.core_services().gameta_client.as_ref();
            crate::settings::gameta_connection_status(
                enabled,
                client.is_some(),
                client.and_then(|client| client.last_known_version()),
            )
        })
        .await
    }

    /// Applies `patch` if `patch.expected_revision` still matches the
    /// current revision (`ApplicationErrorKind::Conflict` otherwise),
    /// validates every patched field before any write
    /// (`ApplicationErrorKind::InvalidInput` on the first invalid one),
    /// and persists atomically in the sense that a failure at any point
    /// leaves both disk and this instance's in-memory settings exactly
    /// as they were before the call -- see `runtime::settings_ops::
    /// run_update_settings`'s own doc comment for the precise ordering
    /// this guarantees.
    pub async fn update_settings(
        &self,
        patch: crate::settings::SettingsPatch,
    ) -> Result<crate::settings::SettingsSnapshot, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_update_settings(&inner, patch).await
        })
        .await?
    }

    /// Sets (always overwrites; there is no "clear" -- disable gameta
    /// server integration via [`Self::update_settings`]'s
    /// `network.gameta_server_enabled` instead) the gameta server API
    /// key.
    pub async fn set_gameta_api_key(&self, value: SecretInput) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_set_gameta_api_key(&inner, value).await
        })
        .await?
    }

    /// Sets (`Some`) or clears (`None`) the SOCKS5 proxy password. `None`
    /// is a real, meaningful choice here (an unauthenticated proxy)
    /// unlike the gameta API key -- see [`Self::set_gameta_api_key`]'s
    /// doc comment for the contrast.
    pub async fn set_socks5_password(
        &self,
        value: Option<SecretInput>,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_set_socks5_password(&inner, value).await
        })
        .await?
    }

    /// Copies the encrypted vault to `destination` and switches this
    /// instance to using the copy, re-opening it in place. Unlike
    /// [`Self::update_settings`]'s `security.secrets_database_path`
    /// field (which repoints to a path the caller asserts already holds
    /// a usable vault), this physically moves the *current* vault's
    /// contents.
    pub async fn move_vault(
        &self,
        destination: std::path::PathBuf,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_move_vault(&inner, destination).await
        })
        .await?
    }

    /// Re-encrypts the vault under a new key file and switches this
    /// instance to using it.
    pub async fn rekey_vault(&self, key_file: std::path::PathBuf) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_rekey_vault(&inner, key_file).await
        })
        .await?
    }

    /// Every configured password rule's non-secret shape -- never the
    /// stored passwords themselves (see [`crate::settings::
    /// PasswordRuleSummary`]'s own doc comment).
    pub async fn password_rules(
        &self,
    ) -> Result<Vec<crate::settings::PasswordRuleSummary>, ApplicationError> {
        self.dispatch_async(|inner| async move { settings_ops::run_password_rules(&inner).await })
            .await?
    }

    /// Creates a new password rule, or updates the existing one with the
    /// same `rule.name` -- see [`crate::settings::PasswordRuleInput`]'s
    /// own doc comment for exactly what `password: None` means in each
    /// case. Returns the full, updated rule list (matching
    /// [`Self::password_rules`]'s shape) so a caller does not need a
    /// separate round trip to refresh its view.
    pub async fn upsert_password_rule(
        &self,
        rule: crate::settings::PasswordRuleInput,
    ) -> Result<Vec<crate::settings::PasswordRuleSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_upsert_password_rule(&inner, rule).await
        })
        .await?
    }

    /// Atomically replaces the complete password-rule list edited by a
    /// frontend. Existing rows are identified by `original_name`, allowing a
    /// rename with `password: None` to preserve the stored secret without
    /// exposing it to the caller. Returns non-secret summaries of the saved
    /// list.
    pub async fn replace_password_rules(
        &self,
        rules: Vec<crate::settings::PasswordRuleEditInput>,
    ) -> Result<Vec<crate::settings::PasswordRuleSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_replace_password_rules(&inner, rules).await
        })
        .await?
    }

    /// Deletes the password rule named `name`.
    /// `ApplicationErrorKind::NotFound` if no rule has that name.
    pub async fn delete_password_rule(
        &self,
        name: String,
    ) -> Result<Vec<crate::settings::PasswordRuleSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_delete_password_rule(&inner, name).await
        })
        .await?
    }

    // ============== Task 10: settings, secrets, vault (end) ==============

    // ============ Task c5: organization rules/profiles (start) ===========
    // Kept in its own clearly-delimited section for the same reason as
    // the sections above and below it: concurrent worktrees also edit
    // this file. Every method here is a thin dispatch wrapper; the logic
    // lives in `crate::organization` (pure DTOs/validation) and
    // `runtime::organization_ops` (the `AppRuntime`-touching execution
    // layer) -- see both modules' own doc comments.

    /// Every saved organization rule -- what decides the organized
    /// *layout* of an archive's contents. Empty (not an error) when no
    /// organization service is configured.
    ///
    /// A summary's `id` is directly usable as
    /// [`crate::operations::OrganizeRequest::rule_id`].
    pub async fn organization_rules(
        &self,
    ) -> Result<Vec<crate::organization::OrganizationRuleSummary>, ApplicationError> {
        self.dispatch_async(|inner| async move {
            organization_ops::run_organization_rules(&inner).await
        })
        .await?
    }

    /// Creates a rule, or updates an existing one -- see
    /// [`crate::organization::OrganizationRuleInput`]'s own doc comment
    /// for exactly which of the two an `id`-less input does. Returns the
    /// full, updated rule list (matching [`Self::organization_rules`]'s
    /// shape) so a caller does not need a second round trip to refresh
    /// its view, the same way [`Self::upsert_password_rule`] does.
    ///
    /// `InvalidInput` for an empty name, an empty move pattern, or a
    /// `trigger.filename_pattern` that is not a valid regular expression
    /// (an uncompilable pattern would otherwise save fine and then
    /// silently never match -- `RuleEngine::matches_trigger` treats a
    /// compile failure as "no match"). `NotFound` when `id` names no
    /// rule.
    pub async fn upsert_organization_rule(
        &self,
        rule: crate::organization::OrganizationRuleInput,
    ) -> Result<Vec<crate::organization::OrganizationRuleSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_upsert_organization_rule(&inner, rule).await
        })
        .await?
    }

    /// Deletes the rule with `rule_id`. `NotFound` when no rule has that
    /// id -- see `runtime::organization_ops::run_delete_organization_rule`
    /// for the one documented case where a delete reports success and
    /// the rule survives.
    pub async fn delete_organization_rule(
        &self,
        rule_id: String,
    ) -> Result<Vec<crate::organization::OrganizationRuleSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_delete_organization_rule(&inner, rule_id).await
        })
        .await?
    }

    /// Every configured archive-output profile (format/compression
    /// preset) -- what decides the organized output's *container*, as
    /// opposed to a rule's layout. For a settings/organize UI to list.
    /// Empty (not an error) if the config database never opened.
    ///
    /// A summary's `id` is directly usable as
    /// [`crate::operations::OrganizeRequest::profile_id`].
    pub async fn organization_profiles(
        &self,
    ) -> Result<Vec<crate::organization::OrganizationProfileSummary>, ApplicationError> {
        self.dispatch_async(|inner| async move {
            organization_ops::run_organization_profiles(&inner).await
        })
        .await?
    }

    /// Creates a profile (`id: None`) or updates one (`id: Some(..)`),
    /// returning the full, updated profile list.
    ///
    /// `InvalidInput` for an empty name, an `output_format` outside the
    /// set `arclain_core` actually supports, a compression level above
    /// 9, or a compression method the chosen format does not offer --
    /// each of which would otherwise be stored and only surface much
    /// later, as a failed pack or as a profile whose reported format and
    /// real format disagree. `NotFound` when `id` names no profile.
    ///
    /// A profile's `is_system` flag is never taken from the caller: see
    /// [`crate::organization::OrganizationProfileInput`]. Setting
    /// `is_default` clears every other profile's default flag; clearing
    /// it on the current default leaves none.
    pub async fn upsert_organization_profile(
        &self,
        profile: crate::organization::OrganizationProfileInput,
    ) -> Result<Vec<crate::organization::OrganizationProfileSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_upsert_organization_profile(&inner, profile).await
        })
        .await?
    }

    /// Deletes the profile with `profile_id`, returning the full,
    /// updated profile list.
    ///
    /// Preserves the storage layer's existing semantics rather than
    /// layering new ones on top, so read
    /// `runtime::organization_ops::run_delete_organization_profile`'s own
    /// doc comment before relying on this: a **system** profile survives
    /// the delete and this still reports success (the returned list shows
    /// it is still there), and deleting the **default** profile does not
    /// promote a replacement -- the configuration is simply left with no
    /// default until something sets one. `NotFound` when no profile has
    /// that id.
    pub async fn delete_organization_profile(
        &self,
        profile_id: String,
    ) -> Result<Vec<crate::organization::OrganizationProfileSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_delete_organization_profile(&inner, profile_id).await
        })
        .await?
    }

    /// Makes `profile_id` the one default profile, clearing every other
    /// profile's default flag. `NotFound` when no profile has that id --
    /// a check that matters: the underlying statement pair clears every
    /// default before setting the named one, so an unvalidated unknown id
    /// would leave no default at all.
    pub async fn set_default_organization_profile(
        &self,
        profile_id: String,
    ) -> Result<Vec<crate::organization::OrganizationProfileSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_set_default_organization_profile(&inner, profile_id).await
        })
        .await?
    }

    /// What organization rule `rule_id` would do to the archive open in
    /// `session_id`: the planned moves, the files the run would generate
    /// or fetch, the resolved template variables, and the integrity
    /// self-check comparing the planned output's file set to the
    /// archive's own.
    ///
    /// # This is not an operation
    ///
    /// An organize panel recomputes this every time the user changes the
    /// selected rule, so it is deliberately *not* built like the
    /// mutating flows: it registers nothing with the operation registry,
    /// mints no [`crate::ids::OperationId`], broadcasts no
    /// [`crate::event::OperationEvent`], and starts no work that
    /// outlives the returned future. Awaiting it runs one small indexed
    /// lookup plus pure in-memory planning and hands back the answer; a
    /// caller that drops the future simply gets nothing, with no
    /// operation left behind to cancel or reap. Nothing is written and
    /// no archive byte is read -- the plan is computed entirely from the
    /// entry index the session already holds, so it stays cheap at
    /// interaction frequency even for a large archive.
    ///
    /// # Why there is no `profile_id` parameter
    ///
    /// [`crate::operations::OrganizeRequest`] carries both a `rule_id`
    /// and a `profile_id` because they decide different things: the rule
    /// decides the organized *layout*, the profile decides the output
    /// archive's *container* (format, compression, solid, header
    /// encryption). A plan is a function of the rule, the archive's
    /// name, its entries, and its metadata -- the profile contributes
    /// nothing to it, and is consumed only when the organize run
    /// actually packs the result. Taking a `profile_id` here would
    /// therefore be a parameter this method must validate (a second
    /// database round trip on a hot path) and then ignore, while telling
    /// every reader that changing the profile invalidates the preview.
    /// It does not. A frontend that wants to show the output's
    /// extension alongside this already has it: `output_format` is on
    /// every [`crate::organization::OrganizationProfileSummary`]
    /// [`Self::organization_profiles`] returned.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown session or rule id. `InvalidInput` when
    /// the rule cannot produce a usable plan for *this* archive -- an
    /// output path escaping the organized root, or two entries planned
    /// onto one destination. That case is a real answer about the
    /// selected rule, not a transport failure: a caller must surface it
    /// rather than continuing to display the previous rule's plan.
    ///
    /// # A quirk carried over verbatim
    ///
    /// [`crate::organization::OrganizeIntegrityDto::file_discrepancy`] is
    /// reported exactly as `arclain_core`'s `IntegrityReport` computes
    /// it, which reduces algebraically to `moved_files -
    /// original_files` and so can never be positive. It is surfaced
    /// unchanged rather than quietly "corrected" here, because fixing it
    /// means changing what that core computation means.
    pub async fn preview_organize_plan(
        &self,
        session_id: ArchiveSessionId,
        rule_id: String,
    ) -> Result<crate::organization::OrganizePlanPreview, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_preview_organize_plan(&inner, session_id, rule_id).await
        })
        .await?
    }

    /// Which saved rules actually apply to the archive open in
    /// `session_id`: the ids of those whose trigger matches it, in the
    /// order [`Self::organization_rules`] lists them.
    ///
    /// What an organize panel preselects from -- it lists every rule
    /// (a user may organize by a rule that would not have fired on its
    /// own) but starts on one that would. Evaluated by `arclain_core`'s
    /// own trigger matcher over the same session entries and metadata
    /// [`Self::preview_organize_plan`] plans from, so the rule a panel
    /// lands on and the plan it then shows agree about the archive.
    ///
    /// Empty (not an error) when no rule matches, or when no
    /// organization service is configured -- the same treatment
    /// [`Self::organization_rules`] gives a missing service.
    /// `NotFound` for an unknown session id.
    pub async fn matching_organization_rule_ids(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<Vec<String>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            organization_ops::run_matching_organization_rule_ids(&inner, session_id).await
        })
        .await?
    }

    /// Every file path the archive open in `session_id` holds, in stable
    /// path-sorted order -- what an organize panel's "Original" side is
    /// built from, so the two trees it shows side by side describe the
    /// same archive.
    ///
    /// Session data, not rule data: this is the archive's own content
    /// regardless of which rule is selected, so a frontend fetches it
    /// once per session rather than per preview. Files only -- the
    /// directories the entry index synthesizes from file paths are
    /// deliberately absent (a folder with no file under it is not part
    /// of what an organize run would move), matching what
    /// [`Self::list_entries`] reports as `EntryKind::File`.
    ///
    /// `O(files)`: materializes and clones the whole list. A caller that
    /// only needs a window of it wants [`Self::list_entries`]'s paging
    /// instead. `NotFound` for an unknown or already-closed session id.
    pub async fn archive_file_paths(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<Vec<String>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let session = inner.archive_sessions().get(session_id).await?;
            Ok(session.all_file_paths())
        })
        .await?
    }

    // ============= Task c5: organization rules/profiles (end) ============

    // ============== Task c7: the Process page's surface (start) ==========
    // Kept in its own clearly-delimited section for the same reason as
    // the sections around it: concurrent worktrees also edit this file.
    // Every method here is a thin dispatch wrapper; the logic lives in
    // `crate::process` (pure DTOs/validation) and `runtime::process_ops`
    // (the `AppRuntime`-touching execution layer) -- see both modules'
    // own doc comments.

    /// Every saved pipeline preset, in stored order, with the shipped
    /// defaults standing in when no presets file exists yet.
    ///
    /// # Built-ins are a seed, not a namespace
    ///
    /// This is the semantics `arclain_core` has always had, preserved
    /// rather than redesigned, and it is worth stating plainly because
    /// "built-in" usually implies protection here and does not:
    ///
    /// * With **no presets file**, this returns the presets the
    ///   application ships, every one flagged
    ///   [`crate::process::PipelinePresetSummary::builtin`].
    /// * The **first save materializes the whole list** -- built-ins
    ///   included, since a frontend saves the list it was handed. From
    ///   then on the file is the only source: built-ins are never
    ///   re-merged into it.
    /// * So a user may edit, rename, shadow or **delete a built-in, and
    ///   it stays deleted**. Deleting every preset leaves genuinely
    ///   none; the shipped ones do not reappear on the next launch.
    /// * A corrupt or unreadable presets file is *not* an error: it
    ///   degrades to the built-ins (see
    ///   `runtime::process_ops::load_presets`), and the next save
    ///   overwrites it.
    ///
    /// A summary's `name` is directly usable as
    /// [`crate::operations::pipeline::PipelineSpecDto::Preset::id`] --
    /// the presets listed here are read from the same file, through the
    /// same resolution ([`AppPaths::presets_file`]), that
    /// [`Self::start_pipeline`] resolves a preset against.
    pub async fn pipeline_presets(
        &self,
    ) -> Result<Vec<crate::process::PipelinePresetSummary>, ApplicationError> {
        self.dispatch_async(|inner| async move { process_ops::run_pipeline_presets(&inner).await })
            .await?
    }

    /// Creates a preset, or replaces the existing one with the same
    /// name, returning the full updated list (matching
    /// [`Self::pipeline_presets`]'s shape) so a caller does not need a
    /// second round trip -- the same way [`Self::upsert_organization_rule`]
    /// does.
    ///
    /// The name is the key and a save replaces in place; see
    /// [`crate::process::PipelinePresetInput`] for why, and note the
    /// consequence of the built-in rule above: saving over a shipped
    /// preset's name replaces it permanently.
    ///
    /// `InvalidInput` for a blank name, an empty step list, or a step
    /// whose own fields do not parse (an unknown convert format, a
    /// non-numeric organize rule id) -- each of which would otherwise be
    /// stored happily and only fail on the first run that used the
    /// preset. `Persistence` when the presets file cannot be written.
    pub async fn save_pipeline_preset(
        &self,
        preset: crate::process::PipelinePresetInput,
    ) -> Result<Vec<crate::process::PipelinePresetSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            process_ops::run_save_pipeline_preset(&inner, preset).await
        })
        .await?
    }

    /// Deletes the preset named `name`, returning the full updated list.
    ///
    /// `NotFound` when no preset has that name. Deleting a shipped
    /// preset is permitted and permanent -- see
    /// [`Self::pipeline_presets`].
    ///
    /// One case worth knowing: with no presets file yet, the list a
    /// caller sees is the shipped defaults, and deleting one of them
    /// *creates* the file holding the remaining ones. That is exactly
    /// what the pre-facade page did, and it is why deletion sticks.
    pub async fn delete_pipeline_preset(
        &self,
        name: String,
    ) -> Result<Vec<crate::process::PipelinePresetSummary>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            process_ops::run_delete_pipeline_preset(&inner, name).await
        })
        .await?
    }

    /// What a pipeline would do to each of its inputs: the steps in
    /// order, the predicted output path, and any warning about that
    /// path already existing.
    ///
    /// # This is not an operation
    ///
    /// A step editor recomputes this on every edit, so it is
    /// deliberately not built like the mutating flows: it registers
    /// nothing with the operation registry, mints no
    /// [`crate::ids::OperationId`], broadcasts no
    /// [`crate::event::OperationEvent`], and starts no work that
    /// outlives the returned future. Nothing is written, no archive is
    /// opened, and no archive byte is read.
    ///
    /// It is not *free*, though, and the cost is worth knowing: it stats
    /// each predicted output to answer the collision warning, a folder
    /// input reads that directory, and a
    /// [`crate::operations::pipeline::PipelineSpecDto::Preset`] reads the
    /// presets file. All of it runs on the blocking pool and is awaited
    /// before this returns. An editor recomputing per keystroke should
    /// pass [`crate::operations::pipeline::PipelineSpecDto::Steps`],
    /// which is what its own state already is.
    ///
    /// # It predicts what the run does, from the same two ladders
    ///
    /// Every prediction here is computed the way
    /// [`Self::start_pipeline`] computes the real thing, and the two
    /// places that decide a predicted path are resolved deliberately,
    /// not incidentally:
    ///
    /// * **Metadata: per input, from the DLsite library.** The predicted
    ///   name is `arclain_core`'s `stem_from` -- a sanitized metadata
    ///   title if there is one, else a product code detected in the
    ///   input's own file name, else its stem. This resolves the
    ///   metadata half **separately for every input**, through the same
    ///   library lookup keyed on that input's own file name that
    ///   `arclain_core`'s executor performs, by calling the same
    ///   function. That is why this method takes no metadata parameter:
    ///   there is nothing a caller could supply that the run would also
    ///   be given.
    /// * **Collision policy: the request's, else the pipeline's, else
    ///   the setting.** `arclain_core`'s preview resolves an unset
    ///   policy to a hardcoded `Smart` while its executor resolves one
    ///   to the application-wide `default_collision_policy`, so a
    ///   profile that changed that setting had the preview warn about a
    ///   failure the run did not have. This completes the ladder before
    ///   asking core, so the warning describes the policy that will
    ///   actually apply.
    ///
    /// An earlier shape of this method took one caller-supplied archive
    /// session's plugin metadata and applied it to every input, which is
    /// what the pre-facade Process page did (it passed the active tab's
    /// fetched metadata, whatever files the pipeline was pointed at).
    /// That is wrong for a batch in a way worth recording: one blob
    /// across N inputs makes the name derivation return the *same* name
    /// for all N, so the preview shows N outputs collapsed onto one
    /// path. Per-input resolution is not merely the more faithful
    /// choice, it is the only one that can describe a batch at all.
    ///
    /// The two ends can disagree at all only because plugin-reported
    /// session metadata is never persisted into the library the executor
    /// reads. Making the preview honest does not fix that; a plugin's
    /// freshly fetched title still will not name an output until it
    /// reaches the library.
    ///
    /// # What a preview accepts that a run does not
    ///
    /// Two asymmetries with [`crate::operations::PipelineRequest`], both
    /// deliberate, because a half-built pipeline still has to be
    /// previewable: **no inputs** and **no steps** are accepted here and
    /// refused there. Core answers both with a `global_warnings` entry
    /// rather than an error, which is what an editor shows before the
    /// user has finished. Inputs themselves are the same
    /// [`crate::operations::pipeline::PipelineInputsDto`] a run takes,
    /// folder variant included.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown preset id. `InvalidInput` for a
    /// malformed ad-hoc step (an unknown convert format, a non-numeric
    /// organize rule id) -- the one thing a preview validates as
    /// strictly as a run, since a step that cannot be translated has no
    /// behavior to predict.
    pub async fn preview_pipeline(
        &self,
        request: crate::process::PipelinePreviewRequest,
    ) -> Result<crate::process::PipelinePreviewDto, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            process_ops::run_preview_pipeline(&inner, request).await
        })
        .await?
    }

    /// Pipeline runs a previous process started and never finished --
    /// what a "previous runs were interrupted" banner is built from.
    ///
    /// Not [`Self::recent_operations`]: that is this process's own
    /// in-memory operation registry, emptied by a restart. This is
    /// database-persisted and survives one, which is the entire point.
    ///
    /// # What marks a run interrupted
    ///
    /// `arclain_core` records every pipeline run in the config database
    /// and leaves the row `in_progress` while it executes. At startup,
    /// composing the database services sweeps any `in_progress` row
    /// whose `started_at` is **more than an hour old** into `failed`
    /// with the marker this query selects on, stamping `completed_at`
    /// with the sweep's own clock. A row can only be in that state
    /// because the process that owned it died, so the sweep is the
    /// definition of "interrupted".
    ///
    /// Two consequences of that hour-long threshold, stated because they
    /// are real: a run interrupted less than an hour before the next
    /// launch is *not* reported (its row is swept by some later
    /// startup instead), and a genuinely long-running pipeline in
    /// another live Arclain process is mislabelled interrupted if a
    /// second instance starts while it is past the hour.
    ///
    /// # `since_unix`, `limit`, and the fact that nothing clears these
    ///
    /// `since_unix` filters on the *sweep* time above -- when the run was
    /// declared interrupted -- not when it started or when it actually
    /// died, which nothing records. Passing `0` therefore means **every
    /// interrupted run ever recorded in this profile**, and that set only
    /// ever grows: no code path deletes these rows, clears the marker, or
    /// acknowledges them. A caller that wants "since I last looked" must
    /// remember a timestamp itself and pass it here; one that passes `0`
    /// and renders a banner will render that banner on every launch,
    /// forever, however long ago the crash was.
    ///
    /// `limit` bounds the answer, the same way
    /// [`Self::recent_operations`]'s does, and exists precisely because
    /// of the growth above -- a caller that only wants a count or the
    /// last handful must not be handed an unbounded list. Rows come
    /// newest-first, so a bounded read keeps the ones worth showing.
    /// **The bound is on the answer, not on the query**: `arclain_db`'s
    /// statement has no `LIMIT`, so the full matching set is still
    /// materialized inside the database layer before being truncated
    /// here. Bounding it properly needs a `crates/db` change.
    ///
    /// Empty (not an error) when no configuration database is open.
    pub async fn interrupted_pipeline_runs(
        &self,
        since_unix: i64,
        limit: u32,
    ) -> Result<Vec<crate::process::InterruptedPipelineRunDto>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            process_ops::run_interrupted_pipeline_runs(&inner, since_unix, limit).await
        })
        .await?
    }

    // =============== Task c7: the Process page's surface (end) ===========

    // ============= chrome layout and display options (start) =============
    // Kept in its own clearly-delimited section for the same reason as the
    // sections around it: concurrent worktrees also edit this file. Every
    // method here is a thin dispatch wrapper; the logic lives in
    // `crate::layout` (pure DTOs/validation) and `runtime::layout_ops`
    // (the `AppRuntime`-touching execution layer) -- see both modules'
    // own doc comments.

    /// Every arrangeable item of one chrome region, in the order it is
    /// stored (`sort_order` ascending) -- what a toolbar, context menu,
    /// tools dialog or info panel renders, and what a layout editor
    /// edits.
    ///
    /// Empty (not an error) when no configuration database is open,
    /// matching [`Self::organization_rules`]'s treatment of the same
    /// situation. Hidden items are included: `visible` is a property of
    /// an item, and an editor needs the hidden ones to offer them back.
    pub async fn list_ui_items(
        &self,
        region: crate::layout::UiRegionDto,
    ) -> Result<Vec<crate::layout::UiItemDto>, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            layout_ops::run_list_ui_items(&inner, region).await
        })
        .await?
    }

    /// Writes `items` into `region`.
    ///
    /// **Upsert, never replace.** Every submitted item is stored (creating
    /// or updating its row); every *other* row in the region is left
    /// exactly as it was. An item is hidden by saving it with
    /// `visible: false`, not by omitting it -- omitting it means "I have
    /// nothing to say about this one". That is what lets a frontend save a
    /// filtered subset of a region without destroying the rows it chose
    /// not to show (the info-panel editor does exactly that with one
    /// host-managed section).
    ///
    /// **Last write wins.** There is no revision to submit and no
    /// conflict to resolve: unlike [`Self::update_settings`], this mirrors
    /// the plain per-row upsert the layout has always been stored with, so
    /// two callers saving one region concurrently end with whichever
    /// finished last. Each individual call is still serialized end to end
    /// against every other configuration mutation, so a save is never
    /// interleaved with another one.
    ///
    /// `InvalidInput` when an item names a different `region` than the one
    /// being written, when an `id` is empty, when two items share an `id`,
    /// or when the batch or one of its text fields exceeds the bounds
    /// documented on [`crate::layout::MAX_UI_ITEMS_PER_REGION`] and
    /// [`crate::layout::MAX_UI_ITEM_TEXT_BYTES`]. Nothing is written when
    /// any item is refused -- validation runs over the whole batch first.
    /// An empty batch is accepted and writes nothing.
    pub async fn save_ui_items(
        &self,
        region: crate::layout::UiRegionDto,
        items: Vec<crate::layout::UiItemDto>,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            layout_ops::run_save_ui_items(&inner, region, items).await
        })
        .await?
    }

    /// The chrome display options: which view the browser opens on,
    /// whether each side panel starts open and how wide, and whether
    /// header buttons carry text labels.
    ///
    /// An option that has never been set reads as its default, so a fresh
    /// profile answers the same as a seeded one. `Unsupported` when no
    /// configuration database is open -- deliberately an error rather than
    /// defaults, because defaults would be indistinguishable from
    /// preferences the user actually chose (see
    /// `runtime::layout_ops::run_ui_display_options`).
    pub async fn ui_display_options(
        &self,
    ) -> Result<crate::layout::UiDisplayOptionsDto, ApplicationError> {
        self.dispatch_async(|inner| async move { layout_ops::run_ui_display_options(&inner).await })
            .await?
    }

    /// Writes every display option at once. Last write wins, for the same
    /// reason [`Self::save_ui_items`] does.
    ///
    /// `InvalidInput` for a panel width that is not a finite,
    /// non-negative number of pixels within
    /// [`crate::layout::MAX_UI_PANEL_WIDTH_PX`]; nothing is written in
    /// that case. Every other field is a bool or a closed enum, so no
    /// other value can be wrong.
    pub async fn save_ui_display_options(
        &self,
        options: crate::layout::UiDisplayOptionsDto,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            layout_ops::run_save_ui_display_options(&inner, options).await
        })
        .await?
    }

    // ============== chrome layout and display options (end) ==============

    // ==================== Task 11: plugin sessions (start) ====================
    // Kept in its own clearly-delimited section for the same reason as the
    // Task 9 sections above: a concurrent worktree may be touching this
    // same shared file for an unrelated task. See `crate::plugins`'s own
    // module doc comment for what this section does and does not port
    // from the pre-facade `crates/ui` plugin-UI machinery.

    /// Every plugin the application's plugin runtime knows about,
    /// successfully loaded or not (see `arclain_plugins::manager::
    /// FailedPlugin`). Empty if the plugin runtime itself is unavailable
    /// (no `ApplicationError` -- an application with no working plugin
    /// runtime simply reports zero plugins, matching `capabilities()`'s
    /// own `plugins_available` flag rather than failing every plugin
    /// call outright).
    pub async fn plugins(&self) -> Result<Vec<crate::plugins::PluginSummary>, ApplicationError> {
        self.dispatch(|inner| {
            let visibility = inner
                .session
                .mutable
                .read()
                .user_config
                .plugin_visibility
                .clone();
            match inner.plugin_manager() {
                Some(manager) => {
                    crate::plugins::PluginSessionStore::plugins(&manager, visibility.as_deref())
                }
                None => Vec::new(),
            }
        })
        .await
    }

    /// Enables or disables one plugin, durably: applies the toggle to the
    /// live `PluginManager` and persists the result so it survives a
    /// restart (see `runtime::settings_ops::run_set_plugin_enabled`'s own
    /// doc comment). `NotFound` for an unknown `plugin_id`.
    ///
    /// Disabling takes effect immediately for *every* plugin surface, not
    /// only for the ones a frontend remembers to stop asking for: a
    /// disabled plugin can no longer open a session, answer
    /// [`Self::plugin_ui_document`], or accept a
    /// [`Self::start_plugin_action`]. Sessions already open are kept but
    /// refuse until the plugin is enabled again -- see `crate::plugins::
    /// PluginSessionStore`'s own doc comment for that policy and its
    /// in-flight edges.
    pub async fn set_plugin_enabled(
        &self,
        plugin_id: String,
        enabled: bool,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_set_plugin_enabled(&inner, plugin_id, enabled).await
        })
        .await?
    }

    /// Persists `settings` as `plugin_id`'s own key/value settings bag
    /// (see `runtime::settings_ops::run_set_plugin_settings`'s own doc
    /// comment).
    pub async fn set_plugin_settings(
        &self,
        plugin_id: String,
        settings: std::collections::HashMap<String, String>,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_set_plugin_settings(&inner, plugin_id, settings).await
        })
        .await?
    }

    /// Opens a fresh renderer-neutral session with `plugin_id`'s
    /// requested `extension_point` (`MainPage`, `Panel`, `PluginButton`,
    /// `Dialog(id)`, or `Page(id)` -- every current WIT extension point),
    /// fetching and normalizing its first document. Rejects a
    /// structurally invalid `Dialog`/`Page` id as `InvalidInput` before
    /// ever reaching the plugin manager. Runs the plugin's
    /// `get-ui-layout` WASM call on this app's own runtime, never the
    /// caller's.
    ///
    /// A *structurally* valid `Dialog`/`Page` id the plugin itself does
    /// not implement still opens successfully: `get-ui-layout` has no way
    /// to report "unknown extension point" versus "known, currently
    /// empty" (both return the same `PluginLayout::Single(vec![])`), so
    /// the resulting document's root is a `Single` node with no children
    /// either way -- indistinguishable from a real, intentionally empty
    /// layout. This matches the pre-facade egui renderer's own behavior
    /// for the same case (it never treated an empty layout as an error
    /// either); a frontend that wants to detect "this plugin has nothing
    /// registered for this dialog/page" needs its own convention (an
    /// empty layout as a sentinel), not a signal this facade can add
    /// without changing the WIT ABI.
    ///
    /// A *disabled* plugin cannot be opened at all: opening runs
    /// `get-ui-layout` in the guest, so the enabled flag is enforced here
    /// rather than left to each renderer to remember. The refusal is
    /// `PermissionDenied`, recognized by
    /// [`crate::plugins::is_plugin_disabled_refusal`] and deliberately
    /// distinct from the `NotFound` an unknown `plugin_id` produces -- a
    /// renderer should quietly draw nothing for the first and drop a
    /// stale reference for the second.
    pub async fn open_plugin_session(
        &self,
        plugin_id: String,
        extension_point: crate::plugins::PluginExtensionPointDto,
    ) -> Result<crate::plugins::PluginSessionSnapshot, ApplicationError> {
        self.open_plugin_session_inner(plugin_id, extension_point, None)
            .await
    }

    /// [`Self::open_plugin_session`], with the caller naming the archive
    /// session this plugin session belongs to instead of letting it be
    /// inferred.
    ///
    /// A frontend that scopes a plugin surface to an archive (an archive
    /// browser's panel) already knows which one, and knows it *before*
    /// this application has necessarily been told which archive is active
    /// -- that report is asynchronous. Inferring the pin from the
    /// application's own active-session state therefore races the report,
    /// and losing that race pins the session to the previously-active
    /// archive, or to none. Naming it removes the inference.
    ///
    /// `None` is not "use the active session": it means the caller's
    /// surface genuinely has no archive origin, and a background fetch it
    /// later requests falls back to whatever is active at completion.
    /// Callers that want the inferred behavior use
    /// [`Self::open_plugin_session`].
    pub async fn open_plugin_session_for_archive(
        &self,
        plugin_id: String,
        extension_point: crate::plugins::PluginExtensionPointDto,
        archive_session_id: Option<ArchiveSessionId>,
    ) -> Result<crate::plugins::PluginSessionSnapshot, ApplicationError> {
        self.open_plugin_session_inner(plugin_id, extension_point, Some(archive_session_id))
            .await
    }

    /// `pinned` is `Some(choice)` when the caller named the origin (even
    /// `Some(None)`, meaning "explicitly no archive"), and `None` when it
    /// wants this application's own active session used instead.
    async fn open_plugin_session_inner(
        &self,
        plugin_id: String,
        extension_point: crate::plugins::PluginExtensionPointDto,
        pinned: Option<Option<ArchiveSessionId>>,
    ) -> Result<crate::plugins::PluginSessionSnapshot, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let manager = crate::plugins::require_manager(inner.plugin_manager())?;
            let Some(handle) = inner.tokio_handle() else {
                return Err(shutdown_error());
            };
            // The archive session pinned to this plugin session for the
            // life of it -- see `PluginSessionStore::open`'s own doc
            // comment for why a background fetch this session later
            // requests must land there rather than on whichever session
            // happens to be active when the fetch finishes.
            let pinned_archive_session =
                pinned.unwrap_or_else(|| inner.active_archive_session().get());
            let opened = inner
                .plugin_sessions()
                .open(
                    manager,
                    plugin_id.clone(),
                    extension_point,
                    pinned_archive_session,
                    &handle,
                )
                .await;
            // `get-ui-layout` ran in the guest, and a guest may write a
            // setting from anywhere -- so the pull happens whether the
            // open succeeded or failed, and costs nothing when the guest
            // wrote nothing (see `settings_ops::flush_plugin_settings`).
            settings_ops::flush_plugin_settings(&inner, &plugin_id).await;
            opened
        })
        .await?
    }

    /// The archive session a plugin session's background metadata writes
    /// are pinned to, as named (or inferred) when it opened -- see
    /// [`Self::open_plugin_session_for_archive`]. `NotFound` for an
    /// unknown or already-closed session id.
    pub async fn plugin_session_archive_origin(
        &self,
        session_id: crate::ids::PluginSessionId,
    ) -> Result<Option<ArchiveSessionId>, ApplicationError> {
        self.dispatch(move |inner| inner.plugin_sessions().pinned_archive_session(session_id))
            .await?
    }

    /// Immediate, in-memory query of the last document revision
    /// retained for `session_id` -- no plugin call. `NotFound` for an
    /// unknown or already-closed session id.
    ///
    /// Refuses, without dropping the session, while the session's plugin
    /// is disabled: the retained document is that plugin's own authored
    /// content, so serving it would leave a disabled plugin's panel on
    /// screen. The refusal is `PermissionDenied` and is recognized by
    /// [`crate::plugins::is_plugin_disabled_refusal`], so a renderer can
    /// distinguish it from the `NotFound` of an unknown session and draw
    /// nothing rather than an error. See `crate::plugins::
    /// PluginSessionStore`'s own doc comment for what a disable does to an
    /// open session, and what it deliberately does not do.
    pub async fn plugin_ui_document(
        &self,
        session_id: PluginSessionId,
    ) -> Result<crate::plugins::PluginUiDocument, ApplicationError> {
        self.dispatch(move |inner| {
            let manager = crate::plugins::require_manager(inner.plugin_manager())?;
            inner.plugin_sessions().document(&manager, session_id)
        })
        .await?
    }

    /// Closes an open plugin session. `NotFound` if `session_id` is
    /// unknown or already closed.
    pub async fn close_plugin_session(
        &self,
        session_id: PluginSessionId,
    ) -> Result<(), ApplicationError> {
        self.dispatch(move |inner| inner.plugin_sessions().close(session_id))
            .await?
    }

    /// Starts dispatching one plugin interaction as a cancellable,
    /// event-broadcasting operation. Returns as soon as the operation is
    /// recorded `Accepted`; the WASM call, any bounded-action
    /// resolution, and re-normalization all happen on a task spawned
    /// through this app's own runtime. Subscribe via
    /// [`Self::subscribe_operations`] to observe `Started` /
    /// `Completed { PluginUiUpdated }` / `Failed`. See `crate::plugins`'s
    /// module doc comment for per-plugin action serialization and the
    /// hidden/disabled node rejection this performs before ever reaching
    /// the WASM guest.
    ///
    /// An action against a *disabled* plugin's session fails the
    /// operation -- `Failed` carrying the same `PermissionDenied` refusal
    /// [`crate::plugins::is_plugin_disabled_refusal`] recognizes, in the
    /// same place an unknown session id fails it, rather than a
    /// request-level `Err`. A disable that lands mid-call fails it too:
    /// the guest call cannot be recalled, but nothing it produced is
    /// published (see `crate::plugins::PluginSessionStore`'s own doc
    /// comment).
    pub async fn start_plugin_action(
        &self,
        request: crate::plugins::PluginActionRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, _cancel) =
                inner.operations().begin(OperationKind::PluginAction).await;
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(async move {
                    let manager =
                        match crate::plugins::require_manager(worker_inner.plugin_manager()) {
                            Ok(manager) => manager,
                            Err(error) => {
                                let _ = worker_inner
                                    .operations()
                                    .transition(operation_id, OperationState::Failed { error })
                                    .await;
                                return;
                            }
                        };
                    // `tokio_handle()` returning `None` here would mean the
                    // runtime finished tearing down in the instant between
                    // this task being spawned and this line -- see
                    // `AppRuntime::tokio_handle`'s doc comment for why that
                    // is only a theoretical race in a real bootstrapped app.
                    // Matches `archive_ops::run_open_archive`'s identical
                    // handle-recheck-mid-worker pattern: there is nothing
                    // left to run on a runtime that is tearing down, so the
                    // operation is left at whatever state it last reached
                    // (`Accepted`, since `Started` has not been recorded
                    // yet) rather than forcing a `Failed` transition onto a
                    // registry that may itself be going away.
                    let Some(worker_handle) = worker_inner.tokio_handle() else {
                        return;
                    };
                    let _ = worker_inner
                        .operations()
                        .transition(operation_id, OperationState::Started)
                        .await;
                    let plugin_id = worker_inner
                        .plugin_sessions()
                        .session_plugin_id(request.session_id);
                    let outcome = worker_inner
                        .plugin_sessions()
                        .dispatch_action(manager, request, &worker_handle)
                        .await;
                    // Before the terminal transition, so a frontend that
                    // reacts to `Completed` is reacting to a state whose
                    // settings are already durable. Runs on the failure
                    // path too: a guest can write a setting and then trap,
                    // and losing the user's setting because the plugin
                    // misbehaved afterwards would be the worse outcome.
                    if let Some(plugin_id) = plugin_id {
                        settings_ops::flush_plugin_settings(&worker_inner, &plugin_id).await;
                    }
                    let state = match outcome {
                        Ok(update) => OperationState::Completed {
                            result: OperationResult::PluginUiUpdated { update },
                        },
                        Err(error) => OperationState::Failed { error },
                    };
                    let _ = worker_inner
                        .operations()
                        .transition(operation_id, state)
                        .await;
                });
            }
            operation_id
        })
        .await
    }

    /// Reports which archive session (if any) is currently active, so
    /// plugin host functions resolve `current_archive_info`/
    /// `list_archive_files`/panel-driven `emit_metadata` against it
    /// instead of a UI-owned notion of "the active tab". A frontend calls
    /// this on every tab-activation change (including "no tab has an
    /// archive open", `None`).
    pub async fn set_active_archive_session(
        &self,
        session_id: Option<ArchiveSessionId>,
    ) -> Result<(), ApplicationError> {
        self.dispatch(move |inner| inner.active_archive_session().set(session_id))
            .await
    }

    /// Installs this application's active-tab bridge on its plugin
    /// runtime, including every plugin instance that is already loaded.
    ///
    /// The bridge resolves archive-backed host calls through the
    /// renderer-neutral session selected by
    /// [`Self::set_active_archive_session`]. `fallback` is invoked only
    /// for a panel-driven metadata write when no archive session is
    /// active, because resolving a frontend's own current tab is the one
    /// responsibility the application cannot own.
    ///
    /// Returns `false` when bootstrap degraded cleanly without a plugin
    /// runtime; there is nothing to install in that case. A frontend can
    /// therefore perform this one-time setup without receiving or
    /// depending on `PluginManager` itself.
    pub fn install_active_tab_bridge(
        &self,
        fallback: impl Fn(Option<serde_json::Value>) + Send + Sync + 'static,
    ) -> Result<bool, ApplicationError> {
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        let Some(manager) = self.inner.plugin_manager() else {
            return Ok(false);
        };
        let bridge = self.active_tab_bridge(fallback);
        manager.lock().set_active_tab_bridge(bridge);
        Ok(true)
    }

    /// This application's own `arclain_plugins::ActiveTabBridge`
    /// implementation, resolved through archive-session state (see
    /// [`Self::set_active_archive_session`]) instead of a UI signal tree,
    /// composed with `fallback` for the one case that state alone cannot
    /// resolve: a panel-driven metadata emit with no archive session
    /// active at all. `fallback` receives exactly that metadata payload
    /// whenever `active_archive_session_id()` is `None` at call time --
    /// see `crate::plugins::ProductionActiveTabBridge`'s own doc comment
    /// for why this crate cannot resolve that case itself (it would mean
    /// knowing about a frontend's own notion of "the active tab", which
    /// this crate must never depend on directly).
    ///
    /// Frontends should use [`Self::install_active_tab_bridge`], which
    /// keeps the plugin runtime behind the application boundary. This
    /// lower-level constructor remains useful to application tests and
    /// other headless composition code that needs to exercise the bridge
    /// directly.
    pub fn active_tab_bridge(
        &self,
        fallback: impl Fn(Option<serde_json::Value>) + Send + Sync + 'static,
    ) -> Arc<dyn arclain_plugins::ActiveTabBridge> {
        Arc::new(crate::plugins::ProductionActiveTabBridge::new(
            crate::plugins::ArchiveContextBridge::new(
                self.inner.active_archive_session().clone(),
                self.inner.archive_sessions_handle(),
            ),
            fallback,
        ))
    }

    /// Resolves a [`crate::plugins::PluginUiDocument`] node's encoded
    /// image `cache_key`/`image_key` value into its cached bytes, capped
    /// at [`crate::plugins::MAX_PLUGIN_IMAGE_BYTES`]. `NotFound` for an
    /// unrecognized or uncached key; `Internal` for a cached entry that
    /// exceeds the cap or any other cache read failure.
    pub async fn read_plugin_image(&self, cache_key: String) -> Result<Vec<u8>, ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::read_plugin_image(image_cache(inner)?, &cache_key)
        })
        .await?
    }

    /// Caches image bytes a caller already holds under the plugin
    /// namespace `cache_key` names -- the write counterpart of
    /// [`Self::read_plugin_image`].
    ///
    /// Exists because that read decodes the owning plugin out of the key:
    /// a frontend writing the same bytes into its own cache namespace
    /// produces an entry the read can never find, so the image stays
    /// broken forever and the entry is orphaned. See
    /// `crate::plugins::write_plugin_image` for the full rationale and
    /// the size cap it shares with the read.
    /// `plugin_id` is the caller's own statement of which plugin's
    /// document referenced `cache_key`, and must match the owner encoded
    /// in the key itself. The key alone is not authority: it names its own
    /// namespace, so accepting it unchecked would let any caller holding a
    /// `plugin-image:victim:k` string write bytes `victim` would later
    /// render as its own.
    ///
    /// **No in-tree caller today, and retained deliberately.** The egui
    /// frontend recovers a missing plugin asset through
    /// [`Self::fetch_plugin_image`], which owns the HTTP request as well as
    /// the write. This method is kept for two reasons that are about the
    /// shape of the surface, not about a hypothetical future:
    ///
    /// - It is the symmetric write half of the namespaced pair. The
    ///   *pairing* of a read and a write that resolve one key to one
    ///   namespace is what makes a namespace mismatch unrepresentable here
    ///   (see [`crate::plugins::authorize_plugin_image_write`], shared by
    ///   this and the fetch). Leaving a `read` with no matching `write`
    ///   re-opens that asymmetry conceptually and invites the next writer
    ///   to invent their own path into the namespace.
    /// - A frontend that obtains bytes out of band -- a bridge fetching
    ///   through its platform's own HTTP stack rather than this one -- has
    ///   no other supported way to put them where the read will look.
    pub async fn write_plugin_image(
        &self,
        plugin_id: String,
        cache_key: String,
        bytes: Vec<u8>,
        source_url: Option<String>,
    ) -> Result<(), ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::write_plugin_image(
                image_cache(inner)?,
                &plugin_id,
                &cache_key,
                &bytes,
                source_url.as_deref(),
            )
        })
        .await?
    }

    // ==================== Task 11: plugin sessions (end) ====================

    // ============ Task 12: host-owned display images (start) ================
    // The host half of the image surface, so a frontend holds neither a
    // content-cache handle nor an HTTP client of its own. See
    // `crate::plugins`'s "Display images" section for why the two image
    // namespaces refuse each other's keys.

    /// Resolves a **host-owned** image cache key into its cached bytes,
    /// capped at [`crate::plugins::MAX_HOST_IMAGE_BYTES`].
    ///
    /// `NotFound` for a key nothing cached; `PermissionDenied` for a
    /// plugin-scoped key (those resolve through
    /// [`Self::read_plugin_image`] and nothing else); `Internal` for an
    /// entry over the cap or any other cache read failure.
    pub async fn read_host_image(&self, cache_key: String) -> Result<Vec<u8>, ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::read_host_image(image_cache(inner)?, &cache_key)
        })
        .await?
    }

    /// Drops a host-owned cached image, reporting whether anything was
    /// removed.
    ///
    /// For the one case a frontend cannot recover from on its own: an entry
    /// that reads back fine but does not decode. Without this it would be
    /// re-read, re-failed, and re-served forever. Plugin-scoped keys are
    /// refused -- evicting a plugin's own cache entry is not a frontend's
    /// call.
    pub async fn discard_host_image(&self, cache_key: String) -> Result<bool, ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::discard_host_image(image_cache(inner)?, &cache_key)
        })
        .await?
    }

    /// Serves a **host-owned** image cache key, fetching `url` into it
    /// first when it is not cached yet, and reports which of the two
    /// happened.
    ///
    /// This is the affordance that lets a frontend stop owning an HTTP
    /// client: it passes the key its renderer asked for plus the URL the
    /// document offered as a fallback, and the application owns the
    /// fetch, the [`crate::plugins::MAX_HOST_IMAGE_BYTES`] ceiling
    /// (enforced *while* reading the body, so an oversized response is
    /// never buffered whole), the response validation, and the cache
    /// write. A second call for the same key answers from the cache with
    /// no network request at all.
    ///
    /// `on_behalf_of_plugin` names whose domain whitelist and rate limit
    /// gate the request -- a plugin document's URL fallback must spend that
    /// plugin's network budget even for a legacy host-owned key. It is
    /// **not** a namespace selector: this method writes the host namespace
    /// only, and refuses a plugin-scoped `cache_key` with
    /// `PermissionDenied`.
    pub async fn fetch_host_image(
        &self,
        cache_key: String,
        url: String,
        on_behalf_of_plugin: Option<String>,
    ) -> Result<crate::plugins::ImageBytesDto, ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::fetch_host_image(
                image_cache(inner)?,
                &inner.core_services().async_http_client,
                &cache_key,
                &url,
                on_behalf_of_plugin.as_deref(),
            )
        })
        .await?
    }

    /// Serves a **plugin-scoped** image cache key, fetching `url` into the
    /// plugin's own cache namespace first when it is not cached yet.
    ///
    /// The fetch-and-cache counterpart of [`Self::read_plugin_image`], and
    /// the reason a frontend never needs [`Self::write_plugin_image`] to
    /// recover a missing plugin asset: this spends `plugin_id`'s network
    /// budget and writes `plugin_id`'s namespace, both derived from the one
    /// authorization the write path uses. `plugin_id` must match the owner
    /// the key encodes (`PermissionDenied` otherwise), and a key this
    /// facade never encoded is `NotFound` -- host-owned keys go through
    /// [`Self::fetch_host_image`].
    pub async fn fetch_plugin_image(
        &self,
        plugin_id: String,
        cache_key: String,
        url: String,
    ) -> Result<crate::plugins::ImageBytesDto, ApplicationError> {
        self.dispatch_blocking(move |inner| {
            crate::plugins::fetch_plugin_image(
                image_cache(inner)?,
                &inner.core_services().async_http_client,
                &plugin_id,
                &cache_key,
                &url,
            )
        })
        .await?
    }

    /// Runs `work` on the application's **blocking** pool.
    ///
    /// [`Self::dispatch`]'s counterpart for work that genuinely blocks.
    /// `dispatch` spawns onto a runtime *worker*, which is right for a
    /// cheap state read and wrong for anything touching a disk or an HTTP
    /// client:
    ///
    /// - A cache read opens a content-addressed blob, streams up to 50 MiB
    ///   out of it and updates an index row; a discard takes the cache's
    ///   key and root locks. On a worker that does not panic, it *occupies*
    ///   one -- and image traffic is the highest-volume caller this facade
    ///   has, so enough of it starves the workers the operation registry
    ///   and session-event bridge run on.
    /// - A fetch additionally drives a client that calls `block_on`
    ///   internally, which from a worker thread does panic.
    ///
    /// Being the only route these methods have to their work is what stops
    /// the blocking hop from being something a future call site forgets.
    async fn dispatch_blocking<T, F>(&self, work: F) -> Result<T, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(&AppRuntime) -> T + Send + 'static,
    {
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        let Some(handle) = self.inner.tokio_handle() else {
            return Err(shutdown_error());
        };
        let inner = self.inner.clone();
        handle
            .spawn_blocking(move || work(&inner))
            .await
            .map_err(|join_error| {
                ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                    .with_diagnostic(join_error.to_string())
            })
    }

    /// The byte ceiling this application applies to one fully materialized
    /// resource body -- the bound a caller must respect when it asks a
    /// remote service for a whole payload rather than streaming it.
    ///
    /// Synchronous on purpose. It reads an immutable value this instance
    /// resolved at bootstrap (no I/O, no lock, no runtime hop), and its
    /// callers are render-path code: handing them a future would push them
    /// straight into a `block_on` on the UI thread, which is exactly the
    /// pattern this facade exists to remove. `ArclainApp::api_version` and
    /// `ArclainApp::active_tab_bridge` are synchronous for the same
    /// reason.
    pub fn materialized_resource_limit(&self) -> usize {
        self.inner.materialized_resource_limit()
    }

    // ============= Task 12: host-owned display images (end) =================

    // ============ Task 14n: boundary-zero network surface (start) ===========
    // The application-owned halves of the network surface a frontend used
    // to reach `arclain-network` directly for. The remaining piece
    // (`crate::plugins::analyze_url`) needs no application state at all
    // and is a free function rather than a method here.

    /// Every domain `plugin_id` has requested network access to, approved
    /// or still pending, in a stable domain-sorted order.
    ///
    /// Reads the one live whitelist this application composed at
    /// bootstrap -- the same store the plugin HTTP client consults on
    /// every request -- so what a frontend renders is what the network
    /// layer will actually enforce, not a snapshot that can drift from
    /// it. An unknown (but non-empty) `plugin_id` is not an error: it
    /// simply has requested no domains yet.
    pub async fn plugin_domain_whitelist(
        &self,
        plugin_id: String,
    ) -> Result<Vec<crate::plugins::DomainWhitelistEntryDto>, ApplicationError> {
        self.dispatch(move |inner| {
            let whitelist = inner.core_services().domain_whitelist.read();
            crate::plugins::plugin_domain_whitelist(&whitelist, &plugin_id)
        })
        .await?
    }

    /// Approves or revokes one domain requested by `plugin_id`.
    ///
    /// The decision is persisted before the live HTTP policy changes, and
    /// concurrent mutations are serialized so durable and in-memory
    /// state cannot observe opposite orderings. A failed persistence
    /// write leaves live network access unchanged.
    pub async fn set_plugin_domain_approved(
        &self,
        plugin_id: String,
        domain: String,
        approved: bool,
    ) -> Result<(), ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_set_plugin_domain_approved(&inner, plugin_id, domain, approved).await
        })
        .await?
    }

    /// Health-checks a *candidate* gameta server configuration -- the
    /// values a settings form currently holds, before the user has saved
    /// them -- and reports what the server said about itself.
    ///
    /// Persists nothing: neither `server_url` nor `api_key` touches the
    /// configuration database, the encrypted vault, or this instance's
    /// live `gameta_client`. Use [`Self::update_settings`] plus
    /// [`Self::set_gameta_api_key`] to actually save a configuration.
    ///
    /// `Ok` means the server is reachable and speaking the gameta
    /// protocol: it answered `/api/v1/health` with a success status and a
    /// body that parsed into the expected `{status, version}` shape. The
    /// returned [`crate::settings::GametaServerInfo`] carries both of
    /// those fields verbatim. The `status` **value** is not interpreted --
    /// a server reporting `"degraded"` still returns `Ok` here, and a
    /// frontend that wants to react to that reads the field itself.
    ///
    /// # `api_key` is accepted but not currently transmitted
    ///
    /// `/api/v1/health` is an unauthenticated endpoint, and
    /// `arclain_network`'s client sends no `Authorization` header for it
    /// -- so **a wrong or expired key still reports success here**. This
    /// probe validates reachability and protocol, not credentials. The
    /// parameter is taken now (and as a zeroizing
    /// [`crate::challenge::SecretInput`], matching
    /// [`Self::set_gameta_api_key`]) so that the day an authenticated
    /// probe endpoint exists, wiring it changes only this method's body
    /// and not every caller. Long-standing behavior, not a regression:
    /// the settings page worked exactly this way before this surface
    /// existed.
    ///
    /// Failures carry a redaction-safe summary: the user-typed
    /// `server_url` may appear in it (with any embedded userinfo stripped
    /// -- see `settings_ops::redact_url_userinfo`), the `api_key` never
    /// does.
    pub async fn test_gameta_connection(
        &self,
        server_url: String,
        api_key: Option<SecretInput>,
    ) -> Result<crate::settings::GametaServerInfo, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_test_gameta_connection(&inner, server_url, api_key).await
        })
        .await?
    }

    /// Probes the *candidate* network path -- the values a settings form
    /// currently holds, before the user has saved them -- by sending a
    /// real outbound request and reporting every step it took.
    ///
    /// `proxy: None` probes the **direct** path: no proxy, reporting the
    /// egress the machine actually has. `Some(candidate)` routes the same
    /// request through the candidate SOCKS5 settings. Those are the two
    /// halves of a settings page's "test connection" button -- "does my
    /// proxy work" and "what does the internet see without it".
    ///
    /// Persists nothing and touches no live routing: neither the
    /// candidate nor its password reaches the configuration database, the
    /// encrypted vault, or this instance's `AsyncHttpClient`. Use
    /// [`Self::update_settings`] plus [`Self::set_socks5_password`] to
    /// actually save a proxy.
    ///
    /// # `Ok` does not mean the probe succeeded
    ///
    /// A probe that *ran* returns `Ok` whatever it found, and
    /// [`crate::settings::NetworkProbeReport::succeeded`] reports the
    /// verdict. The trace is the point: a frontend renders which step
    /// failed and why, which an `Err` carrying one summary string cannot
    /// express. `Err` is reserved for a candidate that could never have
    /// been probed at all (`InvalidInput` for a host/port pair that
    /// cannot form an authority -- rejected before any packet leaves) or
    /// an application already shutting down.
    ///
    /// The candidate `password` appears nowhere in the report: every
    /// string a step carries is credential-free at its source (see
    /// `settings_ops::probe_report`).
    pub async fn probe_network(
        &self,
        proxy: Option<crate::settings::Socks5Candidate>,
    ) -> Result<crate::settings::NetworkProbeReport, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            settings_ops::run_probe_network(&inner, proxy).await
        })
        .await?
    }

    // ============= Task 14n: boundary-zero network surface (end) ============

    // ========== Task P1: plugin management surfaces (start) =================
    // Install, plus the two read models an application frame draws its
    // plugin-owned chrome and its network-diagnostics page from. Kept in
    // its own clearly-delimited section for the same reason as the
    // sections above: a concurrent worktree may be touching this same
    // shared file.

    /// Installs a plugin from a `.wasm` component file, returning the
    /// installed plugin's id.
    ///
    /// Copies the component and a manifest derived from its own metadata
    /// export into this application's plugins directory, then loads and
    /// initializes it -- so the plugin is usable immediately, without a
    /// restart. `InvalidInput` for a request that is malformed on its face
    /// (empty path, a path over
    /// [`crate::plugins::MAX_PLUGIN_INSTALL_PATH_BYTES`], or one that does
    /// not name a `.wasm` file); `Plugin` for a file that is not a valid,
    /// installable plugin -- missing, not a component, declaring an id
    /// that is already installed, failing its own `init`; `Unsupported`
    /// when this application has no plugin runtime at all.
    ///
    /// # Not a registered operation, and not cancellable
    ///
    /// Deliberately a plain awaited `Result` rather than an
    /// [`crate::ids::OperationId`]: `PluginManager::install_plugin` is one
    /// synchronous call with no interior cancellation point and no
    /// progress to report, and it holds the plugin manager's lock from
    /// start to finish, so there is nothing an operation id could observe,
    /// interleave with, or interrupt. Handing one out would advertise a
    /// cancellation this application cannot honor -- and a cancel that
    /// "succeeded" after the component was already published and
    /// registered would be a lie about the state of the user's plugins
    /// directory, not merely a no-op.
    ///
    /// # The installed plugin is enabled for this run only
    ///
    /// A freshly installed plugin starts enabled in the live manager but
    /// is **not** added to the persisted enabled-plugin set. Once that set
    /// exists at all, `bootstrap` trusts it completely and disables
    /// anything absent from it, so a plugin installed and never explicitly
    /// toggled is disabled again at the next start. Call
    /// [`Self::set_plugin_enabled`] after installing to make it durable.
    /// Long-standing behavior, carried over unchanged rather than fixed
    /// here: the pre-facade install path did exactly the same thing.
    pub async fn install_plugin(
        &self,
        wasm_path: std::path::PathBuf,
    ) -> Result<String, ApplicationError> {
        crate::plugins::validate_install_path(&wasm_path)?;
        self.dispatch_blocking(move |inner| {
            let manager = crate::plugins::require_manager(inner.plugin_manager())?;
            crate::plugins::PluginSessionStore::install_plugin(&manager, &wasm_path)
        })
        .await?
    }

    /// The plugin-owned chrome an application frame draws: how many
    /// plugins are loaded and enabled, and every top tab the enabled ones
    /// currently register, sorted by the priority they declare.
    ///
    /// Reads each enabled plugin's tabs live, on this application's
    /// blocking pool -- deliberately *not* through `PluginManager`'s own
    /// memoized top-tab list, which only refreshes when a plugin is
    /// enabled, disabled or loaded and would therefore freeze a badge
    /// count at whatever it was then. That makes this a real call into
    /// every enabled plugin, so a caller that polls owns the cadence; it
    /// is not a per-frame read.
    ///
    /// A plugin whose `get-top-tabs` call fails contributes no tabs and
    /// does not fail this read. An application composed without a plugin
    /// runtime reports zero counts and no tabs rather than an error, for
    /// the same reason [`Self::plugins`] reports an empty list.
    pub async fn plugin_chrome(
        &self,
    ) -> Result<crate::plugins::PluginChromeSnapshot, ApplicationError> {
        self.dispatch_blocking(|inner| match inner.plugin_manager() {
            Some(manager) => crate::plugins::PluginSessionStore::plugin_chrome(&manager),
            None => crate::plugins::PluginChromeSnapshot::default(),
        })
        .await
    }

    /// Every enabled plugin's network-activity log, merged and sorted
    /// oldest first.
    ///
    /// The lines plugins write through the WIT `log-network-activity`
    /// import -- what a diagnostics page renders. Already bounded at the
    /// source (per plugin: at most 256 lines, 256 KiB, each line at most
    /// 4 KiB, oldest evicted first), so this is a whole-log read with no
    /// paging.
    ///
    /// Kept separate from [`Self::plugin_chrome`] rather than folded into
    /// one call because the two have unrelated staleness needs: chrome is
    /// redrawn with the window, a network log matters only while its page
    /// is open, and combining them would impose the tighter cadence on
    /// both. Reports an empty log, not an error, when this application has
    /// no plugin runtime.
    pub async fn plugin_network_log(
        &self,
    ) -> Result<Vec<crate::plugins::PluginNetworkLogEntryDto>, ApplicationError> {
        self.dispatch_blocking(|inner| match inner.plugin_manager() {
            Some(manager) => crate::plugins::PluginSessionStore::plugin_network_log(&manager),
            None => Vec::new(),
        })
        .await
    }

    // =========== Task P1: plugin management surfaces (end) ==================

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

/// The composed content cache every image method needs, or the one error
/// they all report when this instance was composed without one (no cache
/// directory, or a cache index that failed to open at bootstrap).
fn image_cache(inner: &AppRuntime) -> Result<&Arc<arclain_core::ContentCache>, ApplicationError> {
    inner.content_cache().ok_or_else(|| {
        ApplicationError::new(
            ApplicationErrorKind::Unsupported,
            "content cache is unavailable",
        )
        .with_recoverability(Recoverability::Fatal)
    })
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
