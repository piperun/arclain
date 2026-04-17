//! UI-side wrapper that spawns the core pipeline executor on the tokio
//! runtime and routes progress events to the `process_run` signal.

use crate::core::signals::ProcessRunState;
use arclain_core::{execute_pipeline, Pipeline, PipelineProgress};
use arclain_signals::Signal;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::runtime::Runtime;

pub fn spawn_run(
    pipeline: Pipeline,
    state_arc: Arc<Mutex<crate::core::AppState>>,
    signal: Signal<ProcessRunState>,
    runtime: &Runtime,
) {
    let temp_root = std::env::temp_dir();

    // Reset signal to a fresh running state
    let mut initial = ProcessRunState::default();
    initial.is_running = true;
    signal.set(initial);

    runtime.spawn(async move {
        let signal_for_blocking = signal.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let backend_for = |p: &std::path::Path| state_arc.lock().backend_selector.select(p);

            let result = execute_pipeline(
                &pipeline,
                &temp_root,
                backend_for,
                |ev| {
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
                        PipelineProgress::FileFailed { .. } => {
                            s.files_failed += 1;
                        }
                        PipelineProgress::AllComplete { succeeded, failed } => {
                            s.is_running = false;
                            s.completed = true;
                            s.summary = Some(format!(
                                "Done: {} succeeded, {} failed",
                                succeeded, failed
                            ));
                        }
                    }
                    signal_for_blocking.set(s);
                },
            );

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
