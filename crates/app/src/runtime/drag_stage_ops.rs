//! `ArclainApp`'s drag-stage surface: the async `start_drag_stage`
//! operation starter, and `stage_drag_payload_blocking` -- this facade's
//! one deliberately **synchronous** entry point, existing for exactly one
//! caller shape: a foreign OS thread that must block until archive bytes
//! exist on disk.
//!
//! # Why a blocking method belongs on an async facade
//!
//! Windows drag-and-drop hands the shell an `IDataObject`; when the user
//! drops, the shell calls back `GetData` on the drag source's own STA
//! thread and synchronously expects the transfer to be servable. That
//! thread is a plain `std::thread::spawn` thread running a COM modal
//! loop -- it is not, and can never be, a Tokio worker. Forcing that
//! caller through the async surface would mean every drag source
//! open-coding its own `block_on` bridge against `start_drag_stage` +
//! `subscribe_operations` -- the exact "sync-COM-to-async bridge hack"
//! this arc explicitly rejected, scattered in frontend code where its
//! invariants (never on a runtime thread, subscribe-before-start, lag
//! reconciliation, lease renewal) cannot be enforced. Centralizing it
//! here makes the boundary crossing a designed, guarded, tested surface:
//! one choke point, executor-invariant, and documented.
//!
//! # Threading contract (load-bearing)
//!
//! `stage_drag_payload_blocking` must only ever be called from a thread
//! that is **not** running on any Tokio runtime. `Handle::block_on` from
//! inside a runtime task panics ("Cannot start a runtime from within a
//! runtime" / "Cannot block the current thread from within a runtime") --
//! the panic class this workspace already hit once from a runtime-task
//! `block_on`. The method debug-asserts the contract and, in release
//! builds, refuses with a structured `Internal` error instead of letting
//! tokio's own panic surface -- misuse is a caller bug either way, but a
//! refused drop beats an aborted process.

use std::sync::atomic::Ordering;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationKind, OperationResult, OperationState};
use crate::ids::OperationId;
use crate::materialization::drag_stage::{
    run_drag_stage, DragStageEvent, DragStageRequest, DragStagingLease,
};

use super::{shutdown_error, ArclainApp};

fn cancelled_error(operation_id: OperationId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Cancelled,
        "the drag stage was cancelled",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_operation_id(operation_id)
}

fn event_stream_closed_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "the operation event stream closed before the drag stage finished",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn percent_of(completed_units: u64, total_units: Option<u64>) -> u8 {
    match total_units {
        Some(total) if total > 0 => ((completed_units * 100) / total).min(100) as u8,
        _ => completed_units.min(100) as u8,
    }
}

/// Maps a terminal state observed for the drag-stage operation onto the
/// blocking call's own result. `None` for non-terminal states.
fn terminal_outcome(
    operation_id: OperationId,
    state: OperationState,
) -> Option<Result<crate::materialization::MaterializationLease, ApplicationError>> {
    match state {
        OperationState::Completed {
            result: OperationResult::Materialized { lease },
        } => Some(Ok(lease)),
        OperationState::Completed { .. } => Some(Err(ApplicationError::new(
            ApplicationErrorKind::Internal,
            "the drag stage completed with an unexpected result payload",
        )
        .with_operation_id(operation_id))),
        OperationState::Cancelled => Some(Err(cancelled_error(operation_id))),
        OperationState::Failed { error } => Some(Err(error)),
        _ => None,
    }
}

impl ArclainApp {
    /// Starts staging a multi-entry drag selection onto local disk as a
    /// cancellable, event-broadcasting operation
    /// (`OperationKind::DragStage`). Returns as soon as the operation is
    /// recorded `Accepted`; extraction happens on a task spawned through
    /// this app's own runtime handle (see
    /// `crate::materialization::drag_stage::run_drag_stage`). Subscribe
    /// via [`Self::subscribe_operations`] to observe `Started` /
    /// `Progress` (percent out of 100) / `Completed { Materialized }`
    /// (whose lease's `local_path` is the staging **root** directory) /
    /// `Cancelled` / `Failed`. Never raises a `Challenge` -- see the
    /// worker's module doc comment for why a password failure fails fast
    /// here.
    ///
    /// Most drag sources want [`Self::stage_drag_payload_blocking`]
    /// instead, which drives this to completion and returns a
    /// self-renewing lease handle.
    pub async fn start_drag_stage(
        &self,
        request: DragStageRequest,
    ) -> Result<OperationId, ApplicationError> {
        self.dispatch_async(move |inner| async move {
            let (operation_id, cancel) = inner.operations().begin(OperationKind::DragStage).await;
            // See `start_extract`'s identical comment -- the same
            // theoretical, non-reachable-in-practice race.
            if let Some(handle) = inner.tokio_handle() {
                let worker_inner = inner.clone();
                handle.spawn(run_drag_stage(worker_inner, operation_id, cancel, request));
            }
            operation_id
        })
        .await
    }

    /// Synchronously stages a drag selection and blocks until the staged
    /// files exist on disk, returning a [`DragStagingLease`] that keeps
    /// them alive (auto-renewing) until dropped.
    ///
    /// **Must be called from a thread that is not a Tokio runtime worker**
    /// -- see the module doc comment for the full threading contract. The
    /// intended caller is an OS drag source's own STA/COM thread at the
    /// moment the shell commits to a drop.
    ///
    /// `on_event` observes [`DragStageEvent::Started`] (delivering the
    /// `OperationId` another thread can pass to
    /// [`Self::cancel_operation`] to abort the stage while this thread is
    /// blocked) and [`DragStageEvent::Progress`] ticks. Cancellation
    /// unblocks this call with `ApplicationErrorKind::Cancelled`
    /// immediately; the background worker finishes cooperatively and
    /// removes its own staging directory.
    ///
    /// The subscription to the operation stream is taken *before* the
    /// operation starts, so no event can be missed; if the bounded
    /// broadcast still lags under load, the loop reconciles against
    /// [`Self::operation`]'s snapshot rather than trusting the gap.
    pub fn stage_drag_payload_blocking(
        &self,
        request: DragStageRequest,
        on_event: &mut dyn FnMut(DragStageEvent),
    ) -> Result<DragStagingLease, ApplicationError> {
        debug_assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "stage_drag_payload_blocking must never be called from inside a Tokio runtime -- \
             it blocks, and blocking a runtime worker panics tokio; call it from the drag \
             source's own OS thread"
        );
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(ApplicationError::new(
                ApplicationErrorKind::Internal,
                "stage_drag_payload_blocking was called from inside a runtime thread",
            )
            .with_recoverability(Recoverability::Fatal));
        }
        if self.inner.shut_down.load(Ordering::SeqCst) {
            return Err(shutdown_error());
        }
        let Some(handle) = self.inner.tokio_handle() else {
            return Err(shutdown_error());
        };

        let lease = handle.block_on(async {
            // Subscribe first: every event the fresh operation ever
            // publishes lands in this receiver's queue.
            let mut events = self.subscribe_operations();
            let operation_id = self.start_drag_stage(request).await?;
            on_event(DragStageEvent::Started { operation_id });

            loop {
                match events.recv().await {
                    Ok(event) if event.operation_id == operation_id => {
                        if let OperationState::Progress {
                            completed_units,
                            total_units,
                            message,
                        } = &event.state
                        {
                            on_event(DragStageEvent::Progress {
                                percent: percent_of(*completed_units, *total_units),
                                message: message.clone(),
                            });
                            continue;
                        }
                        if let Some(outcome) = terminal_outcome(operation_id, event.state) {
                            return outcome;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Our terminal event may be among the discarded
                        // ones -- reconcile against the snapshot instead
                        // of waiting for an event that already passed.
                        let snapshot = self.operation(operation_id).await?;
                        if let Some(outcome) = terminal_outcome(operation_id, snapshot.state) {
                            return outcome;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(event_stream_closed_error());
                    }
                }
            }
        })?;

        Ok(DragStagingLease::new(self.clone(), lease, handle))
    }
}
