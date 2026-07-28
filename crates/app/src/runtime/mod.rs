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

mod bootstrap;
mod paths;
mod session_store;

pub use bootstrap::BootstrapConfig;
pub use paths::AppPaths;
pub use session_store::{
    AppCapabilities, BackendCapabilityDto, ExternalToolStatusDto, HealthSnapshot, LegacyComposition,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
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

    /// Idempotent: a second call (this wrapper's own `Drop` calls it
    /// too) is a no-op, since the first call already took the `Option`.
    fn shutdown_now(&self) {
        let Some(runtime_arc) = self.0.lock().take() else {
            return;
        };
        match Arc::try_unwrap(runtime_arc) {
            Ok(runtime) => runtime.shutdown_background(),
            Err(_still_shared_with_e_g_a_legacy_services_clone) => {
                // Not the last reference -- dropping it here is a plain
                // refcount decrement, never the operation that reaches
                // `tokio::runtime::Runtime`'s real (blocking) teardown.
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
/// [`RuntimeOwner`]) is declared *last* so it is always the one to
/// observe (via `Arc::try_unwrap` in [`RuntimeOwner::shutdown_now`])
/// whichever reference actually turns out to be the final one, rather
/// than `session`'s bare clone dropping after it and reaching
/// `tokio::runtime::Runtime`'s unprotected `Drop` directly. This fully
/// covers `AppRuntime`'s own lifecycle; see `RuntimeOwner`'s doc
/// comment for the one further-out case (a `Services` clone that
/// escapes via `take_legacy_composition` and outlives every
/// `ArclainApp` clone) this cannot reach.
pub(crate) struct AppRuntime {
    paths: AppPaths,
    session: SessionStore,
    tokio_runtime: RuntimeOwner,
    /// Set once by [`ArclainApp::shutdown`]. Every [`ArclainApp::dispatch`]
    /// call checks this first so a clone that outlives shutdown (held by
    /// another part of the program, or racing a concurrent shutdown call)
    /// gets a structured error instead of silently spawning onto a runtime
    /// that may already be tearing down.
    shut_down: AtomicBool,
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
    /// `AppRuntime`) shut down and tears down the application's Tokio
    /// runtime via [`RuntimeOwner::shutdown_now`]. Every subsequent
    /// facade call on any clone -- including a second `shutdown()` call
    /// racing this one, and including a clone the caller kept around
    /// after this one shut the application down -- goes through
    /// [`Self::dispatch`], which checks the shutdown flag first: a
    /// second `shutdown()` call is a documented no-op success (returns
    /// `Ok(())` without doing anything further), while every *other*
    /// facade method returns a structured `ApplicationError` (`kind:
    /// Internal`) instead of silently spawning onto a runtime that may
    /// already be tearing down.
    ///
    /// There is nothing to drain yet (no in-flight operations exist
    /// until a later task wires the operation registry into
    /// `ArclainApp`), so today this is exactly "mark shut down, tear
    /// down the runtime" -- but callers adopt the right calling
    /// convention now, before there is real cancellation/draining work
    /// for it to do.
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
        // the runtime once it starts tearing down.
        self.inner.tokio_runtime.shutdown_now();
        Ok(())
    }

    /// Transitional handoff of this bootstrap's composed headless
    /// services to `crates/ui`'s not-yet-migrated `AppState`/`Services`
    /// construction. See [`LegacyComposition`]'s doc comment -- this is
    /// not part of the frontend-neutral operation surface a Flutter/Dart
    /// bridge would use.
    pub fn take_legacy_composition(&self) -> LegacyComposition {
        self.inner.session.take_legacy_composition()
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
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        // `shutdown()` may have run concurrently between the check above
        // and here; `RuntimeOwner::handle` returning `None` (rather than
        // panicking or spawning onto a torn-down runtime) is exactly the
        // same "already shut down" outcome, just observed a moment later.
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
