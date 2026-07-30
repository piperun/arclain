//! The drag payload seam between the platform COM layer and the
//! application facade.
//!
//! The Windows drag objects (`super::windows`) never talk to the
//! application directly: they hold a [`DragPayloadSource`] and call
//! [`DragPayloadSource::stage_blocking`] at exactly one moment -- when
//! the shell has committed to a drop and is synchronously waiting inside
//! `IDataObject::GetData` for real files to exist. Until that moment
//! nothing here runs at all, which is precisely how the hover-then-
//! extract optimization survives the facade cutover: hovering a drop
//! target costs a pre-built placeholder HDROP and zero facade calls.
//!
//! [`FacadeDragPayloadSource`] is the production implementation, backed
//! by `ArclainApp::stage_drag_payload_blocking` -- the facade's one
//! synchronous, foreign-thread-only affordance (see
//! `arclain_app::runtime::drag_stage_ops`'s module doc comment for the
//! threading contract it enforces). The COM layer's own tests substitute
//! a fake source, which is what lets "hover never stages" be pinned
//! against the real `IDataObject` state machine without a live shell.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use arclain_app::ids::{ArchiveSessionId, EntryId, OperationId};
use arclain_app::materialization::{DragStageEvent, DragStageRequest};
use arclain_app::ArclainApp;

/// One progress tick of a drag-out, in the shape the drag dialog
/// consumes. Locally owned replacement for the `arclain_core`
/// `ProgressUpdate` the pre-facade drag channels carried -- same fields,
/// no headless-crate dependency.
#[derive(Clone, Debug)]
pub struct DragProgressUpdate {
    pub percent: u8,
    pub message: Option<String>,
}

/// A successfully staged drag payload: the root directory the staged
/// files live under, plus an opaque keep-alive guard that owns them.
///
/// The COM data object stores this for as long as the shell holds the
/// `IDataObject`; dropping it (when the shell releases the object) is
/// what releases the underlying resource -- for the facade-backed
/// source, an `arclain_app::materialization::DragStagingLease` whose
/// drop releases the staged lease directory.
pub struct StagedDragPayload {
    root: PathBuf,
    _keep_alive: Box<dyn std::any::Any + Send + Sync>,
}

impl StagedDragPayload {
    pub fn new(root: PathBuf, keep_alive: impl std::any::Any + Send + Sync) -> Self {
        Self {
            root,
            _keep_alive: Box::new(keep_alive),
        }
    }

    /// The directory every staged path lives under, in the same
    /// archive-relative layout the selection named.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

impl std::fmt::Debug for StagedDragPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedDragPayload")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// What a drag data object needs from the application: the ability to
/// synchronously stage the dragged selection onto disk, once, at drop
/// time -- and to have that stage cancelled from another thread.
pub trait DragPayloadSource: Send + Sync {
    /// Blocks the calling thread until the dragged selection exists on
    /// disk, reporting progress along the way. Called only from a thread
    /// that is not a Tokio runtime worker (the drag STA thread, or a
    /// plain worker thread the progress-dialog pump spawns).
    fn stage_blocking(
        &self,
        on_progress: &mut dyn FnMut(DragProgressUpdate),
    ) -> Result<StagedDragPayload, String>;

    /// Best-effort cancellation of an in-flight (or imminent)
    /// [`Self::stage_blocking`], callable from any thread. A cancelled
    /// stage returns `Err` promptly; cleanup of anything partially
    /// staged is the implementation's own responsibility.
    fn request_cancel(&self);
}

/// The production [`DragPayloadSource`]: stages through
/// `ArclainApp::stage_drag_payload_blocking` and keeps the resulting
/// self-renewing `DragStagingLease` alive inside the returned payload.
pub struct FacadeDragPayloadSource {
    app: ArclainApp,
    /// The application runtime's handle, for the fire-and-forget
    /// `cancel_operation` spawn in [`Self::request_cancel`] -- which must
    /// work from threads (the progress-dialog pump) that cannot block.
    runtime: tokio::runtime::Handle,
    session_id: ArchiveSessionId,
    entry_ids: Vec<EntryId>,
    /// The staging operation's id, set by the `Started` event as soon as
    /// the blocking call registers it -- the handle `request_cancel`
    /// cancels through while `stage_blocking` is still blocked.
    operation_id: parking_lot::Mutex<Option<OperationId>>,
    /// Set by [`Self::request_cancel`]; checked both before starting
    /// (cancel-before-start) and when `Started` lands (closing the race
    /// where cancellation arrives after the flag check but before the
    /// operation id exists to cancel through).
    cancel_requested: AtomicBool,
}

impl FacadeDragPayloadSource {
    pub fn new(
        app: ArclainApp,
        runtime: tokio::runtime::Handle,
        session_id: ArchiveSessionId,
        entry_ids: Vec<EntryId>,
    ) -> Self {
        Self {
            app,
            runtime,
            session_id,
            entry_ids,
            operation_id: parking_lot::Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
        }
    }

    fn spawn_cancel(&self, operation_id: OperationId) {
        let app = self.app.clone();
        self.runtime.spawn(async move {
            let _ = app.cancel_operation(operation_id).await;
        });
    }
}

impl DragPayloadSource for FacadeDragPayloadSource {
    fn stage_blocking(
        &self,
        on_progress: &mut dyn FnMut(DragProgressUpdate),
    ) -> Result<StagedDragPayload, String> {
        if self.cancel_requested.load(Ordering::SeqCst) {
            return Err("drag cancelled before staging began".to_string());
        }
        let request = DragStageRequest {
            session_id: self.session_id,
            entry_ids: self.entry_ids.clone(),
        };
        let staged = self
            .app
            .stage_drag_payload_blocking(request, &mut |event| match event {
                DragStageEvent::Started { operation_id } => {
                    *self.operation_id.lock() = Some(operation_id);
                    // A request_cancel that raced in before the id
                    // existed has nothing to act on -- act on its behalf
                    // now that it does.
                    if self.cancel_requested.load(Ordering::SeqCst) {
                        self.spawn_cancel(operation_id);
                    }
                }
                DragStageEvent::Progress { percent, message } => {
                    on_progress(DragProgressUpdate { percent, message });
                }
            })
            .map_err(|error| match &error.diagnostic {
                Some(diagnostic) => format!("{} ({diagnostic})", error.summary),
                None => error.summary.clone(),
            })?;
        let root = staged.local_root().to_path_buf();
        Ok(StagedDragPayload::new(root, staged))
    }

    fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
        if let Some(operation_id) = *self.operation_id.lock() {
            self.spawn_cancel(operation_id);
        }
    }
}

impl std::fmt::Debug for FacadeDragPayloadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacadeDragPayloadSource")
            .field("session_id", &self.session_id)
            .field("entry_count", &self.entry_ids.len())
            .finish_non_exhaustive()
    }
}
