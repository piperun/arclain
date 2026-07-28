//! The `start_convert`/`start_organize`/`start_pipeline` background
//! workers, and the shared per-file execution loop underneath Convert
//! and Pipeline.
//!
//! # Characterization: what these three operations replace
//!
//! Pre-facade, batch conversion and multi-step processing ran through
//! one place: `crates/ui/src/features/process/view.rs`'s Process page
//! built an `arclain_core::Pipeline` (any mix of `Flatten`/`Organize`/
//! `Convert` steps) and `crates/ui/src/core/operations/
//! process_runner.rs::spawn_run` ran it via `arclain_core::execute_pipeline`
//! on the shared tokio runtime, forwarding `PipelineProgress` into a
//! `Signal` the progress dialog rendered from. `ConvertRequest`/
//! `PipelineRequest` become an `arclain_core::Pipeline` here and run
//! through that exact same `execute_pipeline`, inheriting its existing
//! guarantees unchanged: output-transaction commit/rollback
//! (`arclain_core::features::pipeline::output_transaction::StagedOutput`
//! -- a losing/colliding run never touches an existing, unrecognized
//! destination, proven by that module's own exhaustive test suite),
//! collision policy (`Fail`/`Skip`/`Overwrite`/`Smart`, resolved the
//! same way `process_runner.rs` already did: a per-pipeline override,
//! else the `default_collision_policy` app setting, else `Smart`), and
//! DB-recorded run dedup.
//!
//! `OrganizeRequest` is different, by adjudication (this task's first
//! submission got this wrong -- see `crate::operations::organize`'s own
//! doc comment for the full correction). The pre-facade UI actually had
//! a *second*, older, single-archive "quick action" pair of flows that
//! predate the Pipeline executor and never went through
//! `execute_pipeline`/`StagedOutput` at all:
//! `crates/ui/src/features/organization/presentation/controllers/
//! organization_controller.rs::ActionContext::handle` reads a rule (for
//! layout) and a profile (for output format/compression) from two
//! independent UI selections, builds an `OrganizationPlan` via
//! `RuleEngine::create_plan`, and calls `arclain_core::features::
//! organization::execute_organization_plan(&archive, dest, &plan,
//! temp_dir, profile)` -- a pure core function with **no output
//! transaction at all**: it extracts, applies the plan, and packs
//! straight onto `dest` via `archive.backend().
//! create_archive_with_profile(...)`. `OrganizeRequest` wraps *this*
//! flow, matching the quick action exactly (down to reusing
//! `archive.backend()` -- whichever backend `ctx.backend_for` resolved
//! for extraction -- for the final pack too, unlike Convert's
//! hardcoded, un-overridable `SevenZipCli`). The absence of a
//! transaction is real and preserved, not a bug: a colliding
//! destination for Organize is genuinely at risk, exactly as it always
//! was pre-facade.
//!
//! # Orchestration decisions this task makes
//!
//! - **Per-file invocation (Convert/Pipeline).** `execute_pipeline`
//!   takes a whole `Vec<PathBuf>` and loops internally with *no*
//!   cancellation hook at all -- confirmed by reading its source: the
//!   pre-facade UI's own comment on this exact loop states plainly that
//!   "mid-execution cancellation is not possible with the current
//!   blocking executor" (see `process_runner.rs`). This module instead
//!   calls `execute_pipeline` once *per input file*, checking
//!   `OperationRegistry::is_cancelled` between calls -- preserving the
//!   pre-facade limitation exactly (a file already dispatched runs to
//!   completion) while making that per-file granularity newly
//!   cancellable at the boundary between files. `run_organize` uses the
//!   same per-file loop shape for the same reason, even though its inner
//!   call (`execute_organization_plan`) is a single opaque blocking call
//!   with no progress callback at all.
//! - **Progress translation (Convert/Pipeline).** Each per-file
//!   `execute_pipeline` call's `PipelineProgress` stream is bridged into
//!   `OperationState::Progress` via an unbounded `tokio::sync::mpsc`
//!   channel, drained concurrently with the `spawn_blocking` call
//!   producing it. `completed_units`/`total_units` track *which input
//!   file* (this module's own loop index/total); `message` carries the
//!   finer per-step detail as human text. `StepWarnings` is reported as
//!   one `Progress` event *per warning* (not a count) -- see
//!   `translate_progress`'s own doc comment for why.
//! - **Operation-level terminal state.** Matching `execute_pipeline`'s
//!   own "keep going, tally the outcome" semantics: a per-file failure
//!   is folded into this module's running counters and reported in the
//!   final progress message, but never turns the *operation* `Failed`
//!   -- only a genuine infrastructure failure (the spawned blocking
//!   task itself panicking/joining with an error) does that.
//! - **No `Challenge::ConfirmOverwrite`.** None of the pre-facade flows
//!   this replaces ever raised an interactive per-run overwrite prompt
//!   for a colliding output -- collision handling was, and stays,
//!   resolved by `OutputCollisionPolicy`/direct-overwrite semantics
//!   chosen ahead of time, never a mid-run confirmation dialog.
//! - **`OperationResult::None` only.** The facade contract enumerates
//!   every `OperationResult` variant and which task adds it; none is
//!   attributed to this task. These operations complete with
//!   `OperationResult::None`; the human-readable outcome lives entirely
//!   in the final `OperationState::Progress` message.
//! - **Presets resolve via `AppPaths`, not `arclain_core::
//!   default_presets_path()`.** The latter calls `arclain_app_fs::
//!   AppDirectories::init`, which both ignores `BootstrapConfig::
//!   paths_override` entirely (reading the *real* OS-conventional
//!   directory regardless of what this app instance was told to use)
//!   and *creates* seven directories on disk as a side effect merely by
//!   being called. `resolve_pipeline_spec` instead joins
//!   `AppPaths::config_dir` (already override-aware, already created by
//!   `AppPaths::ensure_created` at bootstrap) with the same
//!   `"pipeline_presets.json"` filename `default_presets_path()` uses --
//!   identical in production, correct under `paths_override`, no extra
//!   side effect.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use arclain_core::backends::BackendSelector;
use arclain_core::services::{LibraryService, OrganizationService};
use arclain_core::{
    execute_pipeline, ArchiveBackend, CompressionLevel, ConvertFormat, GameMetadata,
    OutputArtifact, OutputCollisionPolicy, Pipeline, PipelineContext, PipelineInput,
    PipelineOutput, PipelineProgress, PipelineStep, COLLISION_POLICY_CONFIG_KEY,
};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationResult, OperationState};
use crate::ids::OperationId;
use crate::operations::organize::ParsedIds;
use crate::operations::pipeline::PipelineSpecDto;
use crate::operations::{ConvertRequest, OrganizeRequest};

use super::AppRuntime;

// ─── request -> `arclain_core` translation (Convert/Pipeline) ──────────

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

/// Resolves a [`PipelineSpecDto`] into the full `Pipeline` it names
/// (steps, `output_artifact`, `collision_policy` -- everything except
/// `.input`/`.output`, which [`crate::runtime::ArclainApp::start_pipeline`]
/// overrides with the request's own `inputs`/`destination`).
pub(super) async fn resolve_pipeline_spec(
    inner: &Arc<AppRuntime>,
    spec: &PipelineSpecDto,
) -> Result<Pipeline, ApplicationError> {
    match spec {
        PipelineSpecDto::Preset { id } => resolve_preset_pipeline(inner, id).await,
        PipelineSpecDto::Steps { steps } => {
            let core_steps = steps
                .iter()
                .map(|step| step.to_core())
                .collect::<Result<Vec<_>, _>>()?;
            // Named, narrower gap than this DTO's first draft: the
            // Process page's ad-hoc step builder also has an independent
            // "Output as:" Archive/Folder dropdown
            // (`arclain_core::OutputArtifact`) this DTO does not yet
            // expose as its own field -- see this task's report. Rather
            // than a fixed default regardless of the step list's own
            // shape (this DTO's first draft hardcoded `Archive`
            // unconditionally, which forced even a Flatten-only ad-hoc
            // pipeline through the real, un-overridable 7-Zip pack step
            // for no reason -- caught by this task's own test suite),
            // this derives the artifact kind from whether a `Convert`
            // step is present: packing only makes sense once something
            // has chosen a format to pack *into*; a step list with no
            // `Convert` step has nothing for `execute_pipeline`'s own
            // Archive-mode fallback-to-zip to be more correct than
            // simply leaving the result as a folder.
            let output_artifact = if core_steps
                .iter()
                .any(|step| matches!(step, PipelineStep::Convert { .. }))
            {
                OutputArtifact::Archive
            } else {
                OutputArtifact::Folder
            };
            Ok(Pipeline {
                input: None,
                steps: core_steps,
                output: PipelineOutput::SameFolder,
                collision_policy: None,
                output_artifact,
            })
        }
    }
}

/// Resolves a saved preset by name. I/O (reads the presets file), so
/// this runs inside `spawn_blocking` rather than directly on the
/// calling async task -- see [`resolve_rule_and_profile`] for the same
/// reasoning applied to Organize's own I/O-requiring pre-flight checks.
async fn resolve_preset_pipeline(
    inner: &Arc<AppRuntime>,
    preset_id: &str,
) -> Result<Pipeline, ApplicationError> {
    let presets_path = inner.paths().config_dir.join("pipeline_presets.json");
    let preset_id = preset_id.to_string();
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };
    let presets = handle
        .spawn_blocking(move || {
            if presets_path.exists() {
                arclain_core::load_presets(&presets_path)
            } else {
                arclain_core::builtin_presets()
            }
        })
        .await
        .map_err(internal_join_error)?;

    presets
        .into_iter()
        .find(|preset| preset.name == preset_id)
        .map(|preset| preset.pipeline)
        .ok_or_else(|| preset_not_found_error(&preset_id))
}

// ─── Convert/Pipeline background workers ───────────────────────────────

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

/// The `start_pipeline` background worker. `template` is the already-
/// resolved `Pipeline` with `.output`/`.collision_policy` already
/// overridden by [`crate::runtime::ArclainApp::start_pipeline`]; only
/// `.input` is left for [`run_pipeline_over_inputs`] to fill in per
/// file.
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

/// The shared per-file execution loop underneath `run_convert`/
/// `run_pipeline` -- see this module's own doc comment for why this
/// calls `execute_pipeline` once per input rather than once for the
/// whole batch.
async fn run_pipeline_over_inputs(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    template: &Pipeline,
    inputs: Vec<PathBuf>,
) {
    let total = inputs.len() as u64;
    if total == 0 {
        // `ConvertRequest`/`PipelineRequest::validate` already reject an
        // empty `inputs` before an operation is ever registered, so this
        // is unreachable in practice; handled defensively rather than
        // assumed.
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
///
/// `StepWarnings` is reported as one `Progress` event *per warning*,
/// not a count: the pre-facade progress dialog rendered each warning as
/// its own line (`process_runner.rs`: `for w in warnings { s.warnings.
/// push(format!("{}: {}", w.mod_folder, w.kind.human())) }`, a `Vec<String>`
/// the dialog iterates). `OperationState::Progress` carries one `message:
/// Option<String>`, not a list, so a bridge reproducing that dialog would
/// need to accumulate messages into its own list either way -- emitting
/// one event per warning lets it do exactly that (`push` each message
/// verbatim) with no further facade change, whereas a single joined
/// multi-line message would force the bridge to re-split it apart to
/// recover the list.
async fn translate_progress(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    index: u64,
    total: u64,
    name: &str,
    progress: PipelineProgress,
    outcome: &mut FileOutcome,
) {
    match progress {
        PipelineProgress::FileStart { .. } => {}
        PipelineProgress::StepStart { step_name, .. } => {
            emit_progress(
                inner,
                operation_id,
                index,
                total,
                Some(format!("{name}: {step_name}")),
            )
            .await;
        }
        PipelineProgress::StepProgress { percent } => {
            emit_progress(
                inner,
                operation_id,
                index,
                total,
                Some(format!("{name}: {percent}%")),
            )
            .await;
        }
        PipelineProgress::FileComplete { output } => {
            outcome.succeeded += 1;
            emit_progress(
                inner,
                operation_id,
                index,
                total,
                Some(format!("{name}: done -> {}", output.display())),
            )
            .await;
        }
        PipelineProgress::FileSkipped { reason, .. } => {
            outcome.skipped += 1;
            emit_progress(
                inner,
                operation_id,
                index,
                total,
                Some(format!("{name}: skipped ({reason})")),
            )
            .await;
        }
        PipelineProgress::FileFailed { error } => {
            outcome.failed += 1;
            emit_progress(
                inner,
                operation_id,
                index,
                total,
                Some(format!("{name}: failed ({error})")),
            )
            .await;
        }
        PipelineProgress::AllComplete { .. } => {}
        PipelineProgress::StepWarnings { warnings } => {
            for warning in warnings {
                emit_progress(
                    inner,
                    operation_id,
                    index,
                    total,
                    Some(format!(
                        "{name}: warning: {}: {}",
                        warning.mod_folder,
                        warning.kind.human()
                    )),
                )
                .await;
            }
        }
    }
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
/// `spawn_blocking`.
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
        backend_for: Arc::new(move |path: &Path| {
            resolve_backend(path, &backend_selector, override_backend.as_ref())
        }),
        config_db: services.config_db.clone(),
        default_collision_policy,
    }
}

/// Resolves the extraction backend for one path: `override_backend`
/// (a test-only seam, see `AppRuntime::archive_backend_override`) wins
/// unconditionally when set, otherwise real, extension-based
/// `BackendSelector::select`. Shared by [`build_pipeline_context`]'s
/// closure and [`organize_one_input`], so Convert/Pipeline and Organize
/// resolve a backend identically.
fn resolve_backend(
    path: &Path,
    backend_selector: &BackendSelector,
    override_backend: Option<&Arc<dyn ArchiveBackend>>,
) -> anyhow::Result<Arc<dyn ArchiveBackend>> {
    if let Some(backend) = override_backend {
        return Ok(backend.clone());
    }
    backend_selector.select(path)
}

/// Resolves metadata for `archive_name` via the library service,
/// mirroring `crates/core/src/features/pipeline/executor.rs::
/// resolve_metadata` exactly (that function is private to `arclain_core`,
/// so this is a small, deliberate duplication -- the same trade-off
/// already accepted elsewhere in this facade, e.g. `archive_ops.rs`'s
/// own `is_password_error`).
fn resolve_metadata(
    archive_name: &str,
    library_service: Option<&Arc<LibraryService>>,
) -> Option<GameMetadata> {
    let lib = library_service?;
    let code = arclain_core::utilities::detect_dlsite_code(archive_name)?;
    let id = format!("dlsite:{code}");
    let product = lib.get_metadata(&id).ok().flatten()?;
    let json = serde_json::to_string(&product).ok()?;
    GameMetadata::from_json(&json).ok()
}

/// Picks the output stem for one input, mirroring `crates/core/src/
/// features/pipeline/types.rs::stem_from` exactly (also private to
/// `arclain_core` -- same duplication trade-off as [`resolve_metadata`]):
/// a sanitized metadata title, else a detected product code, else the
/// input's own file stem.
fn stem_from(input: &Path, metadata: Option<&GameMetadata>) -> std::ffi::OsString {
    if let Some(meta) = metadata {
        let title = meta.title.trim();
        if !title.is_empty() {
            let sanitized = arclain_core::utilities::title_filter::sanitize_title(title);
            if !sanitized.is_empty() {
                return std::ffi::OsString::from(sanitized);
            }
        }
    }
    if let Some(name) = input.file_name().and_then(|n| n.to_str()) {
        if let Some(code) = arclain_core::utilities::detect_dlsite_code(name) {
            return std::ffi::OsString::from(code);
        }
    }
    input.file_stem().unwrap_or_default().to_os_string()
}

// ─── Organize: quick-action-style flow, no output transaction ──────────

/// Confirms an [`OrganizeRequest`]'s already-parsed rule id and profile
/// id both name existing, real rows before
/// [`crate::runtime::ArclainApp::start_organize`] registers an operation
/// for them. I/O (two DB reads), so this runs inside `spawn_blocking`
/// rather than directly on the calling async task.
pub(super) async fn resolve_rule_and_profile(
    inner: &Arc<AppRuntime>,
    rule_id: i64,
    profile_id: i64,
) -> Result<(), ApplicationError> {
    let organization_service = inner
        .core_services()
        .organization_service
        .clone()
        .ok_or_else(organize_unavailable_error)?;
    let config_db_path = inner
        .core_services()
        .db_paths
        .as_ref()
        .map(|paths| paths.config_db.clone())
        .ok_or_else(organize_unavailable_error)?;
    let Some(handle) = inner.tokio_handle() else {
        return Err(shutdown_mid_request_error());
    };

    let rule_exists = handle
        .spawn_blocking(move || organization_service.get_domain_rule(rule_id))
        .await
        .map_err(internal_join_error)?
        .map_err(backend_lookup_error)?
        .is_some();
    if !rule_exists {
        return Err(rule_not_found_error(rule_id));
    }

    let profile_exists = handle
        .spawn_blocking(move || {
            arclain_core::features::organization::load_archive_profile(&config_db_path, profile_id)
        })
        .await
        .map_err(internal_join_error)?
        .map_err(backend_lookup_error)?
        .is_some();
    if !profile_exists {
        return Err(profile_not_found_error(profile_id));
    }

    Ok(())
}

/// Everything [`organize_one_input`] needs, resolved once per operation
/// (not per file) and cheaply cloned per file -- mirrors
/// [`build_pipeline_context`]'s own "resolve once, clone per file"
/// shape.
#[derive(Clone)]
struct OrganizeContext {
    backend_selector: BackendSelector,
    override_backend: Option<Arc<dyn ArchiveBackend>>,
    library_service: Option<Arc<LibraryService>>,
    organization_service: Option<Arc<OrganizationService>>,
    config_db_path: Option<PathBuf>,
}

fn build_organize_context(inner: &Arc<AppRuntime>) -> OrganizeContext {
    let services = inner.core_services().clone();
    OrganizeContext {
        backend_selector: inner.backend_selector(),
        override_backend: inner.archive_backend_override(),
        library_service: services.library_service.clone(),
        organization_service: services.organization_service.clone(),
        config_db_path: services
            .db_paths
            .as_ref()
            .map(|paths| paths.config_db.clone()),
    }
}

/// The `start_organize` background worker. `request.dry_run` skips
/// mutation entirely in favor of a real (I/O-backed, but non-mutating)
/// preview -- see [`run_organize_dry_run`].
pub(super) async fn run_organize(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    request: OrganizeRequest,
    parsed_ids: ParsedIds,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }
    if request.dry_run {
        run_organize_dry_run(&inner, operation_id, &request, &parsed_ids).await;
        return;
    }
    run_organize_for_real(&inner, operation_id, &request, &parsed_ids).await;
}

async fn run_organize_for_real(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    request: &OrganizeRequest,
    parsed_ids: &ParsedIds,
) {
    let total = request.inputs.len() as u64;
    let ctx = build_organize_context(inner);
    let temp_root = std::env::temp_dir();
    let destination = request.destination.clone();
    let rule_id = parsed_ids.rule_id;
    let profile_id = parsed_ids.profile_id;

    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut processed = 0u64;

    for (index, input) in request.inputs.iter().cloned().enumerate() {
        if inner.operations().is_cancelled(operation_id).await {
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
            return;
        };

        let ctx_for_blocking = ctx.clone();
        let temp_root_for_blocking = temp_root.clone();
        let destination_for_blocking = destination.clone();
        let input_for_blocking = input.clone();

        let result = handle
            .spawn_blocking(move || {
                organize_one_input(
                    &input_for_blocking,
                    &destination_for_blocking,
                    rule_id,
                    profile_id,
                    &ctx_for_blocking,
                    &temp_root_for_blocking,
                )
            })
            .await;

        processed += 1;
        match result {
            Ok(Ok(dest_path)) => {
                succeeded += 1;
                emit_progress(
                    inner,
                    operation_id,
                    index as u64,
                    total,
                    Some(format!("{name}: done -> {}", dest_path.display())),
                )
                .await;
            }
            Ok(Err(error)) => {
                failed += 1;
                emit_progress(
                    inner,
                    operation_id,
                    index as u64,
                    total,
                    Some(format!("{name}: failed ({error:#})")),
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

    // No "skipped" count here (unlike Convert/Pipeline's three-part
    // summary): Organize has no `OutputCollisionPolicy` concept at all
    // -- `execute_organization_plan` always attempts the write, matching
    // the pre-facade quick action's own behavior.
    let summary = format!("{succeeded} succeeded, {failed} failed");
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

/// One input's full organize: list -> resolve metadata -> build the
/// rule's plan -> resolve the output profile -> pack. Mirrors
/// `organization_controller.rs::ActionContext::handle`'s `Apply` action
/// exactly, generalized from "the one currently-open archive" to an
/// arbitrary input path.
fn organize_one_input(
    input: &Path,
    destination: &Path,
    rule_id: i64,
    profile_id: i64,
    ctx: &OrganizeContext,
    temp_root: &Path,
) -> anyhow::Result<PathBuf> {
    let backend = resolve_backend(input, &ctx.backend_selector, ctx.override_backend.as_ref())?;
    let info = backend
        .list(input, None)
        .context("listing archive contents")?;
    let archive_name = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let metadata = resolve_metadata(&archive_name, ctx.library_service.as_ref());

    let (plan, profile) = build_plan_and_profile(
        &archive_name,
        &info.entries,
        metadata.as_ref(),
        rule_id,
        profile_id,
        ctx,
    )?;

    let stem = stem_from(input, metadata.as_ref());
    let ext = profile.format.extension();
    let dest_path = destination.join(format!("{}.{ext}", stem.to_string_lossy()));

    // Unlike Convert/Pipeline (whose `StagedOutput::new` creates the
    // destination's parent directory as part of staging), the pre-facade
    // quick action this mirrors never needed to: its `dest_path` was
    // always the *source* archive's own directory (`path.with_extension
    // (...)`), which by definition already exists. This facade's
    // `destination` is an independently-chosen folder that may not exist
    // yet, so it is created here explicitly -- the one accommodation
    // this generalization (single archive -> arbitrary batch) needs.
    std::fs::create_dir_all(destination).with_context(|| {
        format!(
            "creating organize destination directory {}",
            destination.display()
        )
    })?;

    let archive = arclain_core::Archive::new(backend, input.to_path_buf());
    arclain_core::features::organization::execute_organization_plan(
        &archive,
        &dest_path,
        &plan,
        temp_root,
        Some(&profile),
    )
    .context("executing organization plan")?;
    Ok(dest_path)
}

/// Resolves the rule + builds its plan, and separately resolves the
/// output profile -- split out from [`organize_one_input`] so
/// [`run_organize_dry_run`] can reuse the exact same plan-building logic
/// without also packing anything.
fn build_plan_and_profile(
    archive_name: &str,
    entries: &[arclain_core::ArchiveEntry],
    metadata: Option<&GameMetadata>,
    rule_id: i64,
    profile_id: i64,
    ctx: &OrganizeContext,
) -> anyhow::Result<(
    arclain_core::features::organization::engine::OrganizationPlan,
    arclain_core::features::organization::ArchiveProfile,
)> {
    let organization_service = ctx
        .organization_service
        .as_ref()
        .context("Organize requires OrganizationService")?;
    let rule = organization_service
        .get_domain_rule(rule_id)
        .context("looking up organization rule")?
        .with_context(|| format!("organization rule #{rule_id} not found"))?;
    let plan = arclain_core::features::organization::engine::RuleEngine::create_plan(
        &rule,
        archive_name,
        entries,
        metadata,
    )
    .context("building organization plan")?;

    let config_db_path = ctx
        .config_db_path
        .as_ref()
        .context("Organize requires a config database")?;
    let profile =
        arclain_core::features::organization::load_archive_profile(config_db_path, profile_id)
            .context("looking up archive profile")?
            .with_context(|| format!("archive profile #{profile_id} not found"))?;

    Ok((plan, profile))
}

/// Computes a real (I/O-backed: lists the archive, resolves metadata,
/// builds the actual rule plan and resolves the actual profile) but
/// non-mutating preview -- unlike Convert/Pipeline's dry run, there is
/// no pure `arclain_core` preview function for this flow to call, since
/// Organize does not build an `arclain_core::Pipeline` at all. Emits one
/// `Progress` message per input describing the plan (move count, root
/// folder, resolved output path using the *same* metadata the real run
/// would resolve, so a dry-run preview never disagrees with what a real
/// run would produce), then `Completed`.
async fn run_organize_dry_run(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    request: &OrganizeRequest,
    parsed_ids: &ParsedIds,
) {
    let total = request.inputs.len() as u64;
    let ctx = build_organize_context(inner);
    let rule_id = parsed_ids.rule_id;
    let profile_id = parsed_ids.profile_id;
    let destination = request.destination.clone();

    for (index, input) in request.inputs.iter().cloned().enumerate() {
        if inner.operations().is_cancelled(operation_id).await {
            return;
        }
        let name = input
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
            .to_string();

        let Some(handle) = inner.tokio_handle() else {
            return;
        };
        let ctx_for_blocking = ctx.clone();
        let input_for_blocking = input.clone();
        let destination_for_blocking = destination.clone();

        let preview = handle
            .spawn_blocking(move || {
                preview_one_input(
                    &input_for_blocking,
                    &destination_for_blocking,
                    rule_id,
                    profile_id,
                    &ctx_for_blocking,
                )
            })
            .await;

        let message = match preview {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => format!("{name}: preview failed ({error:#})"),
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
        };
        emit_progress(inner, operation_id, index as u64, total, Some(message)).await;
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

fn preview_one_input(
    input: &Path,
    destination: &Path,
    rule_id: i64,
    profile_id: i64,
    ctx: &OrganizeContext,
) -> anyhow::Result<String> {
    let backend = resolve_backend(input, &ctx.backend_selector, ctx.override_backend.as_ref())?;
    let info = backend
        .list(input, None)
        .context("listing archive contents")?;
    let archive_name = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let metadata = resolve_metadata(&archive_name, ctx.library_service.as_ref());

    let (plan, profile) = build_plan_and_profile(
        &archive_name,
        &info.entries,
        metadata.as_ref(),
        rule_id,
        profile_id,
        ctx,
    )?;

    let stem = stem_from(input, metadata.as_ref());
    let ext = profile.format.extension();
    let dest_path = destination.join(format!("{}.{ext}", stem.to_string_lossy()));

    Ok(format!(
        "{}: organize via rule {:?} -> {} ({} file move(s), profile {:?})",
        archive_name,
        plan.rule_name,
        dest_path.display(),
        plan.moves.len(),
        profile.name,
    ))
}

// ─── error helpers ──────────────────────────────────────────────────────

fn organize_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "organizing is unavailable: no organization service/database is configured",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn rule_not_found_error(rule_id: i64) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such organization rule")
        .with_diagnostic(format!("rule id {rule_id} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("rule_id")
}

fn profile_not_found_error(profile_id: i64) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such archive profile")
        .with_diagnostic(format!("profile id {profile_id} does not exist"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("profile_id")
}

fn backend_lookup_error(error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Backend,
        "failed to look up organization data",
    )
    .with_diagnostic(format!("{error:#}"))
    .with_recoverability(Recoverability::Retry)
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
