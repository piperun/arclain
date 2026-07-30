//! Running the Process page's pipeline through the application facade.
//!
//! Pre-facade this module called `arclain_core::execute_pipeline`
//! directly inside a `spawn_blocking` on the shared runtime, built its
//! own `PipelineContext` out of `AppState`/`Services`, and translated
//! `PipelineProgress` into a signal. That bypassed the operation
//! registry entirely: the run had no `OperationId`, could not be
//! cancelled once it started (its own comment said so), and never
//! appeared alongside every other operation the application tracks.
//!
//! Now the page dispatches `ArclainApp::start_pipeline` and projects the
//! resulting operation's event stream onto the `process_run` signal.
//! Cancellation goes to `ArclainApp::cancel_operation`, so it reaches
//! the registry that actually owns the work.
//!
//! ## Why this subscribes rather than going through `operation_bridge`
//!
//! The bridge routes an operation's events onto the *tab* that started
//! it, which is what every archive-scoped operation needs. A Process
//! page run is not archive-scoped: its inputs are files chosen from a
//! dialog, its output goes wherever the page's destination points, and
//! its progress belongs to the page's own modal, not to a tab's. So this
//! subscribes to the same stream directly and filters to the one
//! operation it started. It still holds an [`OpGuard`] on the tab that
//! was active at dispatch, which is what makes closing that tab warn
//! about work in flight (and, once confirmed, cancel it — see
//! [`cancel_pipeline_run`]).

use std::sync::Arc;

use arclain_app::event::{OperationEvent, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::operations::PipelineRequest;
use arclain_app::process::PipelinePreviewRequest;
use arclain_app::ArclainApp;
use tokio::sync::broadcast::error::RecvError;

use crate::core::signals::ProcessRunState;
use crate::core::tabs::{OpGuard, TabState};
use crate::shared::SharedState;

/// Starts `request` as an `ArclainApp` pipeline operation and projects
/// its events onto `shared`'s `process_run` signal.
///
/// `request` is the very [`PipelinePreviewRequest`] the page previewed;
/// it becomes the run request through
/// `PipelineRequest::from`, which is the compiler-enforced hand-off
/// between the two (see `arclain_app::process`). The page therefore
/// cannot run a description other than the one it showed.
///
/// Fire-and-forget: returns as soon as the dispatch is spawned.
pub fn start_pipeline_run(
    shared: &SharedState,
    request: PipelinePreviewRequest,
    origin_tab: Arc<TabState>,
) {
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[process] start_pipeline_run: no application facade available");
        return;
    };

    let signal = shared.signals().process_run.clone();
    signal.set(ProcessRunState {
        is_running: true,
        origin_tab: Some(origin_tab.id),
        message: "Starting...".to_string(),
        ..Default::default()
    });

    // Increments the originating tab's `in_flight_ops`; moved into the
    // spawned task and dropped when it ends (success, failure, or a
    // rejected dispatch).
    let guard = OpGuard::new(&origin_tab);
    let runtime = shared.services.tokio_runtime.clone();

    runtime.spawn(async move {
        let _guard = guard;

        // Subscribed *before* dispatching: `start_pipeline` publishes
        // `Accepted` (and, for a fast failure, a terminal state) from a
        // worker that begins running the moment it is spawned, so a
        // receiver created afterwards can miss the whole operation.
        let mut receiver = app.subscribe_operations();

        let operation_id = match app.start_pipeline(PipelineRequest::from(request)).await {
            Ok(operation_id) => operation_id,
            Err(error) => {
                tracing::error!("[process] start_pipeline was rejected: {error:?}");
                signal.update(|state| {
                    state.is_running = false;
                    state.completed = true;
                    state.summary = Some(format!("Pipeline rejected: {}", error.summary));
                });
                return;
            }
        };
        signal.update(|state| state.operation_id = Some(operation_id));

        // The tab may have been force-closed between the dispatch above
        // and here. The pre-facade runner checked this flag once before
        // starting any work and abandoned the run; now the operation
        // exists, so the equivalent is to cancel it for real.
        if origin_tab
            .tab_cancel
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let _ = app.cancel_operation(operation_id).await;
        }

        drive_run(&app, operation_id, &mut receiver, &signal).await;
    });
}

/// Consumes `receiver` until `operation_id` reaches a terminal state,
/// projecting each event onto `signal`.
async fn drive_run(
    app: &ArclainApp,
    operation_id: OperationId,
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    signal: &arclain_app::Signal<ProcessRunState>,
) {
    loop {
        let state = match receiver.recv().await {
            Ok(event) => {
                if event.operation_id != operation_id {
                    continue;
                }
                event.state
            }
            // The stream is bounded and a long batch is chatty, so a
            // slow consumer can genuinely be lagged off it. Ask the
            // registry for the operation's current state instead of
            // guessing; if it is not terminal yet, keep reading.
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!("[process] missed {skipped} operation event(s); reconciling");
                match app.operation(operation_id).await {
                    Ok(snapshot) => snapshot.state,
                    Err(error) => {
                        tracing::error!("[process] could not reconcile after a lag: {error:?}");
                        finish(signal, operation_id, |_| {
                            "Progress reporting was lost".to_string()
                        });
                        return;
                    }
                }
            }
            Err(RecvError::Closed) => {
                tracing::warn!("[process] the operation-event stream closed mid-run");
                finish(signal, operation_id, |_| {
                    "Progress reporting stopped".to_string()
                });
                return;
            }
        };

        match state {
            OperationState::Accepted | OperationState::Started => {}
            OperationState::Progress {
                completed_units,
                total_units,
                message,
            } => {
                signal.update(|run| {
                    run.files_done = completed_units;
                    run.files_total = total_units.unwrap_or(run.files_total);
                    if let Some(message) = message {
                        run.push_message(message);
                    }
                });
            }
            // A pipeline raises neither of these: `start_pipeline` has no
            // password ladder (see this module's own note in the task
            // report) and touches no archive session. Logged rather than
            // silently dropped so a future facade change that starts
            // raising one is visible instead of appearing to hang.
            OperationState::Challenge { challenge } => {
                tracing::warn!(
                    "[process] a pipeline run raised an unhandled challenge: {challenge:?}"
                );
            }
            OperationState::SnapshotChanged { .. } => {}
            OperationState::Completed { .. } => {
                // The operation's own closing progress message already
                // carries the "N succeeded, M skipped, K failed" tally,
                // so it is the summary rather than a second one counted
                // here from re-parsed text.
                finish(signal, operation_id, |run| format!("Done: {}", run.message));
                return;
            }
            OperationState::Cancelled => {
                finish(signal, operation_id, |run| {
                    run.cancelled = true;
                    "Cancelled".to_string()
                });
                return;
            }
            OperationState::Failed { error } => {
                finish(signal, operation_id, |_| {
                    format!("Pipeline failed: {}", error.summary)
                });
                return;
            }
        }
    }
}

/// Seats the terminal state: stops the spinner, records the summary
/// `describe` builds from the run's own final state, and clears the
/// tracked operation so a stale id can never be cancelled.
///
/// `expected` guards against a second run having replaced this one in
/// the signal while this task was finishing: only the run that still
/// owns the slot writes its own ending into it.
fn finish<F>(signal: &arclain_app::Signal<ProcessRunState>, expected: OperationId, describe: F)
where
    F: FnOnce(&mut ProcessRunState) -> String,
{
    signal.update(|run| {
        if run.operation_id != Some(expected) {
            return;
        }
        let summary = describe(run);
        run.is_running = false;
        run.completed = true;
        run.operation_id = None;
        run.summary = Some(summary);
    });
}

/// Cancels the Process page's in-flight run, if any, through the
/// operation registry.
///
/// Used by the page's progress dialog and by the close-tab confirmation
/// (the facade cannot observe a tab closing on its own, so without this
/// the run would keep going with nowhere left to report to). A no-op
/// when no run is in flight.
pub fn cancel_pipeline_run(shared: &SharedState) {
    let Some(operation_id) = shared.signals().process_run.get().operation_id else {
        return;
    };
    let Some(app) = shared.facade.clone() else {
        return;
    };
    shared.signals().process_run.update(|run| {
        run.push_message("Cancelling...".to_string());
    });
    let runtime = shared.services.tokio_runtime.clone();
    runtime.spawn(async move {
        if let Err(error) = app.cancel_operation(operation_id).await {
            tracing::warn!("[process] cancelling the pipeline run failed: {error:?}");
        }
    });
}
