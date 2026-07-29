//! UI-side wrapper that spawns the core pipeline executor on the tokio
//! runtime and routes progress events to the `process_run` signal.

use crate::core::signals::ProcessRunState;
use crate::core::tabs::{OpGuard, TabState};
use arclain_app::Signal;
use arclain_core::{
    execute_pipeline, OutputCollisionPolicy, Pipeline, PipelineContext, PipelineProgress,
    COLLISION_POLICY_CONFIG_KEY,
};
use parking_lot::Mutex;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::runtime::Runtime;

pub fn spawn_run(
    pipeline: Pipeline,
    state_arc: Arc<Mutex<crate::core::AppState>>,
    services: Arc<crate::core::services::Services>,
    signal: Signal<ProcessRunState>,
    runtime: &Runtime,
    origin_tab: Arc<TabState>,
) {
    let temp_root = std::env::temp_dir();

    // Reset signal to a fresh running state
    let mut initial = ProcessRunState::default();
    initial.is_running = true;
    signal.set(initial);

    // Increment in_flight_ops for the originating tab. The guard is moved
    // into the spawned task and dropped when the task completes (success,
    // error, or tab-cancel abort).
    let guard = OpGuard::new(&origin_tab);

    // R4: hoist backend_selector out of the per-file closure. Pre-fix the
    // closure acquired the AppState mutex once per file (1000 files = 1000
    // unnecessary lock acquisitions for a value that's immutable for the
    // duration). Clone the selector once before spawning.
    let backend_selector = state_arc.lock().backend_selector.clone();

    runtime.spawn(async move {
        // Guard held for the lifetime of the entire async task (including
        // the spawn_blocking call below). Drops when this async block exits.
        let _guard = guard;
        let signal_for_blocking = signal.clone();
        let origin_tab_for_blocking = origin_tab.clone();
        let _ = tokio::task::spawn_blocking(move || {
            // Cooperative cancellation: check before starting any work.
            // If the originating tab was force-closed while this task was
            // queued, abort immediately. Mid-execution cancellation is not
            // possible with the current blocking executor (callback returns
            // `()`) — the task runs to completion if it gets past this check.
            if origin_tab_for_blocking.tab_cancel.load(Ordering::SeqCst) {
                let mut s = signal_for_blocking.get();
                s.is_running = false;
                s.completed = true;
                s.summary = Some("Cancelled: originating tab was closed".to_string());
                signal_for_blocking.set(s);
                return;
            }

            let backend_for = move |p: &std::path::Path| backend_selector.select(p);

            let default_policy = services
                .config_service
                .as_ref()
                .and_then(|svc| svc.get(COLLISION_POLICY_CONFIG_KEY).ok().flatten())
                .and_then(|s| OutputCollisionPolicy::from_settings_str(&s));

            let ctx = PipelineContext {
                organization_service: services.organization_service.clone(),
                library_service: services.library_service.clone(),
                backend_for: Arc::new(backend_for),
                config_db: services.config_db.clone(),
                default_collision_policy: default_policy,
            };

            let result = execute_pipeline(&pipeline, &temp_root, &ctx, |ev| {
                let mut s = signal_for_blocking.get();
                match ev {
                    PipelineProgress::FileStart { index, total, name } => {
                        s.is_running = true;
                        s.current_file = name;
                        s.files_total = total;
                        s.files_done = index;
                        s.completed = false;
                    }
                    PipelineProgress::StepStart { step_name, .. } => {
                        s.current_step = step_name;
                        s.step_percent = 0;
                    }
                    PipelineProgress::StepProgress { percent } => {
                        s.step_percent = percent;
                    }
                    PipelineProgress::FileComplete { .. } => {
                        s.files_done += 1;
                    }
                    PipelineProgress::FileSkipped { .. } => {
                        s.files_skipped += 1;
                    }
                    PipelineProgress::FileFailed { .. } => {
                        s.files_failed += 1;
                    }
                    PipelineProgress::StepWarnings { warnings } => {
                        for w in warnings {
                            s.warnings
                                .push(format!("{}: {}", w.mod_folder, w.kind.human()));
                        }
                    }
                    PipelineProgress::AllComplete {
                        succeeded,
                        skipped,
                        failed,
                    } => {
                        s.is_running = false;
                        s.completed = true;
                        s.summary = Some(if skipped > 0 {
                            format!(
                                "Done: {} succeeded, {} skipped (already done), {} failed",
                                succeeded, skipped, failed
                            )
                        } else {
                            format!("Done: {} succeeded, {} failed", succeeded, failed)
                        });
                    }
                }
                signal_for_blocking.set(s);
            });

            if let Err(e) = result {
                let mut s = signal_for_blocking.get();
                s.is_running = false;
                s.completed = true;
                s.summary = Some(format!("Pipeline failed: {}", e));
                signal_for_blocking.set(s);
            }
        })
        .await;
    });
}
