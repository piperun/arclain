use std::sync::Arc;

/// Owns the frontend runtime without ever exposing an owning reference to
/// worker tasks.
///
/// The last `SharedState` clone may be released by a task running on this
/// very executor. Tokio's ordinary `Runtime::drop` panics in that context;
/// `shutdown_background` is explicitly non-blocking and safe there. Handles
/// handed to tasks do not own the runtime, so this wrapper remains the one
/// teardown authority.
struct FrontendRuntimeOwner(Option<tokio::runtime::Runtime>);

impl Drop for FrontendRuntimeOwner {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// Cloneable view of the frontend executor.
///
/// Only a Tokio `Handle` crosses into background coordinators. The owning
/// runtime remains private to [`FrontendRuntimeOwner`], so it can never
/// become an `Arc<Runtime>` whose final reference is released through
/// Tokio's ordinary, blocking drop path by one of its own worker tasks.
pub struct FrontendExecutor {
    handle: tokio::runtime::Handle,
}

impl FrontendExecutor {
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }
}

impl std::ops::Deref for FrontendExecutor {
    type Target = tokio::runtime::Handle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Executor owned by the egui frontend.
///
/// The application facade owns the headless runtime and every backend
/// service. This smaller, independent runtime only drives frontend work:
/// awaiting facade futures, projecting operation streams into signals, and
/// loading images for egui. Keeping it separate means neither this container
/// nor any UI caller can reach into `arclain_core::services::Services`.
pub struct Services {
    pub tokio_runtime: FrontendExecutor,
    _runtime_owner: Arc<FrontendRuntimeOwner>,
}

impl Services {
    /// Builds the bounded runtime used by the production egui frontend.
    ///
    /// It must be multi-threaded because much of the UI work is spawned and
    /// expected to keep progressing between frames; a current-thread runtime
    /// would only advance while a caller was inside `block_on`.
    pub fn production() -> std::io::Result<Self> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map(Self::new)
    }

    /// Wraps a caller-provided frontend runtime. Used by tests so each
    /// fixture controls the executor it creates and tears down.
    pub fn new(runtime: tokio::runtime::Runtime) -> Self {
        let handle = runtime.handle().clone();
        Self {
            tokio_runtime: FrontendExecutor { handle },
            _runtime_owner: Arc::new(FrontendRuntimeOwner(Some(runtime))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Services;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn dropping_the_last_services_clone_on_its_worker_is_safe() {
        let services = Arc::new(Services::new(
            tokio::runtime::Runtime::new().expect("create frontend runtime"),
        ));
        let handle = services.tokio_runtime.handle().clone();
        let worker_owner = services.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();

        handle.spawn(async move {
            entered_tx.send(()).expect("report worker entry");
            let _ = release_rx.await;
            let safe = catch_unwind(AssertUnwindSafe(|| drop(worker_owner))).is_ok();
            let _ = finished_tx.send(safe);
        });

        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker started");
        // The task now holds the final Services/owner reference.
        drop(services);
        release_tx.send(()).expect("release worker");

        assert!(
            finished_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("worker reported runtime teardown"),
            "dropping Services from its own worker must not use Runtime's blocking Drop",
        );
    }
}
