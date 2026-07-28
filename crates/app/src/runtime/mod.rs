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
//! Async consumers should call [`ArclainApp::bootstrap`] and
//! [`ArclainApp::shutdown`] via `spawn_blocking`; nothing in this crate
//! creates a nested Tokio runtime from inside an async context.

mod bootstrap;
mod paths;
mod session_store;

pub use bootstrap::BootstrapConfig;
pub use paths::AppPaths;
pub use session_store::{
    AppCapabilities, BackendCapabilityDto, ExternalToolStatusDto, HealthSnapshot, LegacyComposition,
};

use std::sync::Arc;

use crate::error::{ApplicationError, ApplicationErrorKind};
use session_store::SessionStore;

/// Owns the Tokio runtime and every composed headless service for one
/// running application instance. Never constructed directly -- see
/// [`ArclainApp::bootstrap`].
pub(crate) struct AppRuntime {
    paths: AppPaths,
    tokio_runtime: Arc<tokio::runtime::Runtime>,
    session: SessionStore,
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

    /// Placeholder for graceful shutdown: nothing owned by this task's
    /// composition needs draining yet (no in-flight operations exist
    /// until a later task wires the operation registry into
    /// `ArclainApp`). Kept as an explicit, awaitable method now so
    /// callers adopt the right calling convention (await it before
    /// dropping their last `ArclainApp` clone) before there is
    /// real cancellation/draining work for it to do.
    pub async fn shutdown(&self) -> Result<(), ApplicationError> {
        self.dispatch(|_inner| ()).await
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
        let inner = self.inner.clone();
        self.inner
            .tokio_runtime
            .handle()
            .spawn(async move { work(&inner) })
            .await
            .map_err(|join_error| {
                ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
                    .with_diagnostic(join_error.to_string())
            })
    }
}
