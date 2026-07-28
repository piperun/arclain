//! The `start_convert`/`start_organize`/`start_pipeline` background
//! workers, and the shared per-file execution loop underneath all three.
//!
//! # Characterization: what these three operations replace
//!
//! Pre-facade, batch conversion/organization/multi-step processing all
//! ran through one place: `crates/ui/src/features/process/view.rs`'s
//! Process page built an `arclain_core::Pipeline` (any mix of `Flatten`/
//! `Organize`/`Convert` steps) and `crates/ui/src/core/operations/
//! process_runner.rs::spawn_run` ran it via `arclain_core::execute_pipeline`
//! on the shared tokio runtime, forwarding `PipelineProgress` into a
//! `Signal` the progress dialog rendered from. (`crates/ui/src/features/
//! archive_operations/application/{conversion,organization}.rs` and
//! `crates/ui/src/features/organization/application/operations.rs` are a
//! *second*, older, single-archive "quick action" pair of flows that
//! predate the Pipeline executor and never went through
//! `execute_pipeline`/`StagedOutput` at all -- see `crate::operations::
//! organize`'s own doc comment for why `OrganizeRequest` wraps the
//! Pipeline-executor flow, not this one.)
//!
//! `ConvertRequest`/`OrganizeRequest`/`PipelineRequest` all become an
//! `arclain_core::Pipeline` here and run through the exact same
//! `execute_pipeline`, inheriting its existing guarantees unchanged:
//! output-transaction commit/rollback (`arclain_core::features::
//! pipeline::output_transaction::StagedOutput` -- a losing/colliding run
//! never touches an existing, unrecognized destination, proven by that
//! module's own exhaustive test suite), collision policy (`Fail`/`Skip`/
//! `Overwrite`/`Smart`, resolved the same way `process_runner.rs` already
//! did: a per-pipeline override, else the `pipeline.default_collision_
//! policy` app setting, else `Smart`), and DB-recorded run dedup.
//!
//! # Orchestration decisions this task makes
//!
//! - **Per-file invocation.** `execute_pipeline` takes a whole
//!   `Vec<PathBuf>` and loops internally with *no* cancellation hook at
//!   all -- confirmed by reading its source: the pre-facade UI's own
//!   comment on this exact loop states plainly that "mid-execution
//!   cancellation is not possible with the current blocking executor"
//!   (see `process_runner.rs`). Since a `Pipeline` already carries only
//!   one logical batch, this module instead calls `execute_pipeline`
//!   once *per input file*, from its own loop, checking
//!   `OperationRegistry::is_cancelled` between calls. This preserves the
//!   pre-facade limitation exactly (a file already dispatched runs to
//!   completion; cancellation only ever skips files that have not
//!   started) while making that same, already-existing per-file
//!   granularity newly cancellable at the boundary between files -- a
//!   small, in-scope orchestration improvement, not a change to
//!   `arclain_core`'s executor.
//! - **Progress translation.** Each per-file `execute_pipeline` call's
//!   `PipelineProgress` stream is bridged into `OperationState::Progress`
//!   via an unbounded `tokio::sync::mpsc` channel: the producer side
//!   (the `on_progress` closure) runs inside `spawn_blocking` and never
//!   awaits anything, so `UnboundedSender::send` (a plain, non-blocking
//!   call) is safe there; the consumer side drains it concurrently on
//!   this module's own async task, translating each event into a
//!   `completed_units`/`total_units`/`message` triple -- `completed_units`/
//!   `total_units` track *which input file* (this module's own loop
//!   index/total), `message` carries the finer per-step detail
//!   (`PipelineProgress::StepStart`/`StepProgress`/etc.) as human text.
//! - **Operation-level terminal state.** Matching `execute_pipeline`'s
//!   own "keep going, tally the outcome" semantics (a per-file `Err` is
//!   caught internally and reported as `PipelineProgress::FileFailed`,
//!   never propagated as `execute_pipeline`'s own `Result::Err`): a
//!   per-file failure (bad archive, output collision, etc.) is folded
//!   into this module's running `succeeded`/`skipped`/`failed` counters
//!   and reported in the final progress message, but never turns the
//!   *operation* `Failed` -- only a genuine infrastructure failure
//!   (the spawned blocking task itself panicking/joining with an error)
//!   does that. This mirrors the pre-facade UI, which always reached
//!   `PipelineProgress::AllComplete` and rendered a summary line even
//!   when every file failed.
//! - **Cancellation freezes progress reporting, not the in-flight file's
//!   own work.** `OperationRegistry::transition` silently no-ops once a
//!   record is already terminal (by design -- see its own "terminal
//!   state ignores further transitions" test). Once `cancel_operation`
//!   lands, that flips this operation's record to `Cancelled`
//!   immediately, so every `emit_progress`/`translate_progress` call
//!   this loop makes for the file that was already in flight at that
//!   moment is silently dropped from then on -- but that file's own
//!   `execute_pipeline` call keeps running to its own real completion in
//!   the background regardless (see the point above: this module never
//!   interrupts a dispatched file mid-flight). A caller has no further
//!   facade-visible signal for exactly when that trailing work finishes;
//!   only its filesystem effect (a normal commit, or a preserved-on-
//!   collision destination) is observable afterward. `crates/app/tests/
//!   processing_operations.rs`'s cancellation test proves this by
//!   polling the filesystem rather than the event stream once it
//!   observes `Cancelled`.
//! - **No `Challenge::ConfirmOverwrite`.** None of the three pre-facade
//!   flows this replaces ever raised an interactive per-run overwrite
//!   prompt for a colliding output -- collision handling was, and stays,
//!   resolved by `OutputCollisionPolicy` chosen ahead of time (a
//!   dropdown on the Process page, or the app-wide default setting), not
//!   a mid-run confirmation dialog. Wiring `Challenge::ConfirmOverwrite`
//!   here would be new UX, not a behavior-preserving move, so it is
//!   deliberately not implemented -- see this task's report.
//! - **`OperationResult::None` only.** The facade contract enumerates
//!   every `OperationResult` variant and which task adds it; none is
//!   attributed to Convert/Organize/Pipeline. These three therefore
//!   complete with `OperationResult::None`, exactly like `start_open_archive`
//!   would if it had no snapshot to report -- the human-readable outcome
//!   lives entirely in the final `OperationState::Progress` message, not
//!   in a new terminal payload. This also means this module never edits
//!   `crate::event`, eliminating any merge overlap with a concurrent
//!   task that might extend `OperationResult` for its own operation kind.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arclain_core::{
    execute_pipeline, ArchiveBackend, CompressionLevel, ConvertFormat, OutputArtifact,
    OutputCollisionPolicy, Pipeline, PipelineContext, PipelineInput, PipelineOutput,
    PipelineProgress, PipelineStep, COLLISION_POLICY_CONFIG_KEY,
};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationResult, OperationState};
use crate::ids::OperationId;
use crate::operations::{ConvertRequest, OrganizeRequest};

use super::AppRuntime;

// ─── request -> `arclain_core` translation ─────────────────────────────

/// Builds the `Pipeline` template a [`ConvertRequest`] runs as. `.input`
/// is left `None` -- [`run_pipeline_over_inputs`] fills it in per file.
fn convert_pipeline_template(request: &ConvertRequest, format: ConvertFormat) -> Pipeline {
    let mut steps = Vec::new();
    if request.flatten {
        // Matches the default a user gets from the Process page's own
        // "+ Flatten" button (`crates/ui/src/features/process/view.rs`).
        steps.push(PipelineStep::Flatten {
            strip_common_prefix: true,
            max_depth: 1,
        });
    }
    steps.push(PipelineStep::Convert {
        format,
        compression: CompressionLevel::Normal,
        password: None,
    });
    Pipeline {
        input: None,
        steps,
        output: PipelineOutput::NewFolder(request.destination.clone()),
        collision_policy: None,
        output_artifact: OutputArtifact::Archive,
    }
}

/// Builds the `Pipeline` template an [`OrganizeRequest`] runs as -- a
/// single `Organize` step, `Folder` output (see `crate::operations::
/// organize`'s doc comment for why organizing never repacks an
/// archive). `.input` is left `None` for the same reason as
/// [`convert_pipeline_template`].
fn organize_pipeline_template(request: &OrganizeRequest, rule_id: i64) -> Pipeline {
    Pipeline {
        input: None,
        steps: vec![PipelineStep::Organize { rule_id }],
        output: PipelineOutput::NewFolder(request.destination.clone()),
        collision_policy: None,
        output_artifact: OutputArtifact::Folder,
    }
}

/// Resolves an [`OrganizeRequest`]'s already-parsed rule id against the
/// app's real `OrganizationService`, confirming it names an existing
/// rule before [`crate::runtime::ArclainApp::start_organize`] registers
/// an operation for it. I/O (a DB read), so this -- unlike the purely
/// structural checks in `OrganizeRequest::validate` -- must run inside
/// `spawn_blocking` rather than directly on the calling async task.
pub(super) async fn resolve_rule(
    inner: &Arc<AppRuntime>,
    rule_id: i64,
) -> Result<(), ApplicationError> {
    let organization_service = inner
        .core_services()
        .organization_service
        .clone()
        .ok_or_else(organize_unavailable_error)?;
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };
    let rule = handle
        .spawn_blocking(move || organization_service.get_domain_rule(rule_id))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| {
            ApplicationError::new(
                ApplicationErrorKind::Backend,
                "failed to look up organization rule",
            )
            .with_diagnostic(format!("{error:#}"))
            .with_recoverability(Recoverability::Retry)
        })?;
    if rule.is_none() {
        return Err(rule_not_found_error(rule_id));
    }
    Ok(())
}

/// Resolves a [`PipelineRequest`]'s `preset_id` into the full, saved
/// `Pipeline` it names (steps, `output_artifact`, `collision_policy` --
/// everything except `.input`/`.output`, which the caller overrides with
/// the request's own `inputs`/`destination`, matching how the Process
/// page already treats a loaded preset). I/O (reads the presets file),
/// so this runs inside `spawn_blocking` for the same reason
/// [`resolve_rule`] does.
pub(super) async fn resolve_preset_pipeline(
    inner: &Arc<AppRuntime>,
    preset_id: &str,
) -> Result<Pipeline, ApplicationError> {
    let presets_path = inner
        .presets_path_override()
        .or_else(arclain_core::default_presets_path);
    let preset_id = preset_id.to_string();
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };
    let presets = handle
        .spawn_blocking(move || match presets_path {
            Some(path) => arclain_core::load_presets(&path),
            None => arclain_core::builtin_presets(),
        })
        .await
        .map_err(internal_join_error)?;

    presets
        .into_iter()
        .find(|preset| preset.name == preset_id)
        .map(|preset| preset.pipeline)
        .ok_or_else(|| preset_not_found_error(&preset_id))
}

// ─── background workers ────────────────────────────────────────────────

/// The `start_convert` background worker.
pub(super) async fn run_convert(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    request: ConvertRequest,
    format: ConvertFormat,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }
    let template = convert_pipeline_template(&request, format);
    run_pipeline_over_inputs(&inner, operation_id, &template, request.inputs).await;
}

/// The `start_organize` background worker. `dry_run` skips execution
/// entirely in favor of a pure preview -- see [`run_dry_run_preview`].
pub(super) async fn run_organize(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    request: OrganizeRequest,
    rule_id: i64,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }
    let mut template = organize_pipeline_template(&request, rule_id);
    if request.dry_run {
        template.input = Some(PipelineInput::Files(request.inputs));
        run_dry_run_preview(&inner, operation_id, &template).await;
        return;
    }
    run_pipeline_over_inputs(&inner, operation_id, &template, request.inputs).await;
}

/// The `start_pipeline` background worker. `template` is the already-
/// resolved preset `Pipeline` with `.output` already overridden by
/// [`crate::runtime::ArclainApp::start_pipeline`]; only `.input` is left
/// for [`run_pipeline_over_inputs`] to fill in per file.
pub(super) async fn run_pipeline(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    template: Pipeline,
    inputs: Vec<PathBuf>,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }
    run_pipeline_over_inputs(&inner, operation_id, &template, inputs).await;
}

/// Computes and reports a pure preview (no filesystem mutation at all)
/// via `arclain_core::preview_pipeline_with_metadata` -- one `Progress`
/// message per input, plus any pipeline-wide warning, then `Completed`.
async fn run_dry_run_preview(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    template: &Pipeline,
) {
    let preview = arclain_core::preview_pipeline_with_metadata(template, None);
    let total = preview.entries.len() as u64;
    for (index, entry) in preview.entries.iter().enumerate() {
        let name = entry
            .input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>");
        let mut message = format!("{name}: {}", entry.operations.join(", "));
        if let Some(output) = &entry.expected_output {
            message.push_str(&format!(" -> {}", output.display()));
        }
        for warning in &entry.warnings {
            message.push_str(&format!(" [{warning}]"));
        }
        emit_progress(inner, operation_id, index as u64 + 1, total, Some(message)).await;
    }
    for warning in &preview.global_warnings {
        emit_progress(
            inner,
            operation_id,
            total,
            total,
            Some(format!("warning: {warning}")),
        )
        .await;
    }
    let _ = inner
        .operations()
        .transition(
            operation_id,
            OperationState::Completed {
                result: OperationResult::None,
            },
        )
        .await;
}

/// The shared per-file execution loop underneath `run_convert`/
/// `run_organize`/`run_pipeline` -- see this module's own doc comment
/// for why this calls `execute_pipeline` once per input rather than
/// once for the whole batch.
async fn run_pipeline_over_inputs(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    template: &Pipeline,
    inputs: Vec<PathBuf>,
) {
    let total = inputs.len() as u64;
    if total == 0 {
        // `ConvertRequest`/`OrganizeRequest`/`PipelineRequest::validate`
        // already reject an empty `inputs` before an operation is ever
        // registered, so this is unreachable in practice; handled
        // defensively rather than assumed.
        let _ = inner
            .operations()
            .transition(
                operation_id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await;
        return;
    }

    let ctx = build_pipeline_context(inner);
    let temp_root = std::env::temp_dir();

    let mut succeeded = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    let mut processed = 0u64;

    for (index, input) in inputs.into_iter().enumerate() {
        if inner.operations().is_cancelled(operation_id).await {
            // `OperationRegistry::cancel` already transitioned this
            // operation to `Cancelled` (that is what set this flag) --
            // nothing further to do but stop.
            return;
        }

        let name = input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        emit_progress(
            inner,
            operation_id,
            index as u64,
            total,
            Some(format!("Processing {name}")),
        )
        .await;

        let Some(handle) = inner.tokio_handle() else {
            // The runtime finished tearing down between the flag check
            // above and here -- see `AppRuntime::tokio_handle`'s own doc
            // comment for why this is only a theoretical race in a real
            // bootstrapped app. Stop rather than panic.
            return;
        };

        let mut per_file = template.clone();
        per_file.input = Some(PipelineInput::Files(vec![input]));
        let ctx_for_blocking = ctx.clone();
        let temp_root_for_blocking = temp_root.clone();
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<PipelineProgress>();

        let execute_handle = handle.spawn_blocking(move || {
            execute_pipeline(
                &per_file,
                &temp_root_for_blocking,
                &ctx_for_blocking,
                move |progress| {
                    let _ = progress_tx.send(progress);
                },
            )
        });

        // Drains concurrently with the blocking work above: `progress_rx`
        // yields events live as `execute_pipeline` reports them, and
        // `recv()` returns `None` once `progress_tx` (moved into the
        // blocking closure's `on_progress`) drops -- which happens
        // exactly when `execute_pipeline` itself returns, so this loop
        // and `execute_handle` finish together with no extra
        // synchronization needed.
        let mut file_outcome = FileOutcome::default();
        while let Some(progress) = progress_rx.recv().await {
            translate_progress(
                inner,
                operation_id,
                index as u64,
                total,
                &name,
                progress,
                &mut file_outcome,
            )
            .await;
        }

        processed += 1;
        match execute_handle.await {
            Ok(Ok(())) => {
                succeeded += file_outcome.succeeded;
                skipped += file_outcome.skipped;
                failed += file_outcome.failed;
            }
            Ok(Err(error)) => {
                // `execute_pipeline` itself only returns `Err` for a
                // whole-batch problem (for example, no input at all) --
                // unreachable here since every call carries exactly one
                // `Files(..)` entry -- but folded into `failed` rather
                // than escalated to an operation-level `Failed`, for the
                // same reason a per-file `FileFailed` is (see this
                // module's own doc comment).
                failed += 1;
                emit_progress(
                    inner,
                    operation_id,
                    index as u64,
                    total,
                    Some(format!("{name}: {error:#}")),
                )
                .await;
            }
            Err(join_error) => {
                let _ = inner
                    .operations()
                    .transition(
                        operation_id,
                        OperationState::Failed {
                            error: internal_join_error(join_error).with_operation_id(operation_id),
                        },
                    )
                    .await;
                return;
            }
        }
    }

    let summary = format!("{succeeded} succeeded, {skipped} skipped, {failed} failed");
    emit_progress(inner, operation_id, processed, total, Some(summary)).await;
    let _ = inner
        .operations()
        .transition(
            operation_id,
            OperationState::Completed {
                result: OperationResult::None,
            },
        )
        .await;
}

/// Per-file tally [`translate_progress`] accumulates into, folded into
/// the whole operation's running totals once that file's
/// `spawn_blocking` call returns.
#[derive(Default)]
struct FileOutcome {
    succeeded: u64,
    skipped: u64,
    failed: u64,
}

/// Translates one `PipelineProgress` event from a single file's
/// `execute_pipeline` call into an `OperationState::Progress` transition,
/// folding the file-level outcome (`FileComplete`/`FileSkipped`/
/// `FileFailed`) into `outcome`. `index`/`total` identify *which* input
/// file this is within the whole batch; the inner event's own
/// `FileStart`/`AllComplete` (always `0 of 1`/a single-file tally, since
/// each call processes exactly one input) are redundant with that and
/// intentionally not forwarded.
async fn translate_progress(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    index: u64,
    total: u64,
    name: &str,
    progress: PipelineProgress,
    outcome: &mut FileOutcome,
) {
    let message = match progress {
        PipelineProgress::FileStart { .. } => return,
        PipelineProgress::StepStart { step_name, .. } => Some(format!("{name}: {step_name}")),
        PipelineProgress::StepProgress { percent } => Some(format!("{name}: {percent}%")),
        PipelineProgress::FileComplete { output } => {
            outcome.succeeded += 1;
            Some(format!("{name}: done -> {}", output.display()))
        }
        PipelineProgress::FileSkipped { reason, .. } => {
            outcome.skipped += 1;
            Some(format!("{name}: skipped ({reason})"))
        }
        PipelineProgress::FileFailed { error } => {
            outcome.failed += 1;
            Some(format!("{name}: failed ({error})"))
        }
        PipelineProgress::AllComplete { .. } => return,
        PipelineProgress::StepWarnings { warnings } => {
            Some(format!("{name}: {} warning(s)", warnings.len()))
        }
    };
    emit_progress(inner, operation_id, index, total, message).await;
}

async fn emit_progress(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    completed_units: u64,
    total_units: u64,
    message: Option<String>,
) {
    let _ = inner
        .operations()
        .transition(
            operation_id,
            OperationState::Progress {
                completed_units,
                total_units: Some(total_units),
                message,
            },
        )
        .await;
}

/// Builds the `PipelineContext` every per-file `execute_pipeline` call
/// in this module shares -- one read of the app's composed services and
/// the app-wide default collision-policy setting, matching exactly what
/// `process_runner.rs::spawn_run` used to build inline. A single, cheap,
/// once-per-operation synchronous read (not per file), so this runs
/// directly on the calling async task rather than through
/// `spawn_blocking` -- unlike [`resolve_rule`]/[`resolve_preset_pipeline`],
/// which run once per `start_*` call but touch a slower path (a fresh DB
/// connection acquisition, or file I/O).
fn build_pipeline_context(inner: &Arc<AppRuntime>) -> PipelineContext {
    let services = inner.core_services().clone();
    let backend_selector = inner.backend_selector();
    let override_backend = inner.archive_backend_override();
    let default_collision_policy = services
        .config_service
        .as_ref()
        .and_then(|service| service.get(COLLISION_POLICY_CONFIG_KEY).ok().flatten())
        .and_then(|value| OutputCollisionPolicy::from_settings_str(&value));

    PipelineContext {
        organization_service: services.organization_service.clone(),
        library_service: services.library_service.clone(),
        backend_for: Arc::new(
            move |path: &Path| -> anyhow::Result<Arc<dyn ArchiveBackend>> {
                if let Some(backend) = override_backend.clone() {
                    return Ok(backend);
                }
                backend_selector.select(path)
            },
        ),
        config_db: services.config_db.clone(),
        default_collision_policy,
    }
}

// ─── error helpers ──────────────────────────────────────────────────────

fn organize_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "organizing is unavailable: no organization service is configured",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn rule_not_found_error(rule_id: i64) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such organization rule")
        .with_diagnostic(format!("rule id {rule_id} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("profile_id")
}

fn preset_not_found_error(preset_id: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such pipeline preset")
        .with_diagnostic(format!("no preset named {preset_id:?}"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("preset_id")
}

fn shutdown_mid_request_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "application is shutting down",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
        .with_diagnostic(join_error.to_string())
}
