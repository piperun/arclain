//! `AppRuntime`-touching execution logic for the Process page's surface:
//! saved-preset CRUD, the synchronous pipeline preview, and the
//! interrupted-prior-runs query.
//!
//! [`crate::process`] holds the DTOs and the pure validation/conversion
//! logic this module calls into; `crate::runtime`'s own `impl ArclainApp`
//! exposes the thin dispatch wrappers -- the same layering
//! `crate::organization`/`runtime::organization_ops` uses.
//!
//! Distinct from the similarly-named [`super::processing_ops`], which
//! runs the registered Convert/Organize/Pipeline operations. Nothing
//! here registers an operation. The two do share [`load_presets`], so
//! the presets this surface lists and edits are exactly the ones
//! `start_pipeline` resolves a `PipelineSpecDto::Preset` against.
//!
//! ## Where the rows live
//!
//! Presets are a JSON file under
//! [`crate::AppPaths::presets_file`], not a database table -- the one
//! configuration this facade owns that is not in the config database.
//! That is `arclain_core`'s existing storage choice, preserved; see that
//! accessor's doc comment for why the *path* is resolved here rather
//! than through `arclain_core::default_presets_path`.
//!
//! Interrupted runs, by contrast, are rows in the config database's
//! `pipeline_runs` table, read through the same `Arc<SqliteDb>` handle
//! `arclain_core`'s own pipeline executor uses for run dedup -- not a
//! second connection to the same file.
//!
//! ## Serializing preset writes
//!
//! [`run_save_pipeline_preset`] and [`run_delete_pipeline_preset`] are
//! read-modify-write over one whole file, so two concurrent callers
//! could otherwise interleave into a lost update -- `arclain_core::
//! save_presets` writes atomically (temp file plus rename), which
//! prevents a *torn* file but not a stale one. Both take
//! `AppRuntime::settings_write_lock` for the whole sequence, the same
//! lock and the same reason `runtime::settings_ops` and
//! `runtime::organization_ops` take it for their own read-modify-write
//! pairs. Reading ([`run_pipeline_presets`]) never takes it.
//!
//! ## The preview never becomes an operation
//!
//! [`run_preview_pipeline`] mints no `OperationId`, broadcasts no
//! `OperationEvent`, and spawns nothing that outlives the call. Its one
//! `spawn_blocking` exists because `arclain_core`'s preview touches the
//! filesystem (it stats every predicted output, and expands a folder
//! input by reading the directory) and is awaited before the function
//! returns -- see `crate::runtime::ArclainApp::preview_pipeline`'s own
//! doc comment for why that distinction is load-bearing.

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::process::{
    self, InterruptedPipelineRunDto, PipelinePresetInput, PipelinePresetSummary,
    PipelinePreviewDto, PipelinePreviewRequest,
};

use super::AppRuntime;

// ============================================================================
// Shared resolution helpers.
// ============================================================================

/// This application's own runtime handle -- never the caller's ambient
/// one, per the crate's runtime rules.
fn handle_for(inner: &Arc<AppRuntime>) -> Result<tokio::runtime::Handle, ApplicationError> {
    inner.tokio_handle().ok_or_else(shutdown_mid_request_error)
}

/// Reads the saved presets, on the blocking pool because this is file
/// I/O.
///
/// Delegates to `arclain_core::load_presets`, whose recovery behavior is
/// preserved exactly and matters here: a missing, unreadable, or
/// unparseable presets file yields the **built-in** presets rather than
/// an error or an empty list. So a corrupt file is not a failure a
/// caller sees -- it is silently replaced by the shipped defaults, and
/// the next save overwrites it. That is `arclain_core`'s long-standing
/// behavior (it logs a warning), mirrored rather than tightened: turning
/// it into an error here would make a Process page that has always
/// degraded gracefully suddenly fail to open.
pub(super) async fn load_presets(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<arclain_core::SavedPreset>, ApplicationError> {
    let path = inner.paths().presets_file();
    handle_for(inner)?
        .spawn_blocking(move || arclain_core::load_presets(&path))
        .await
        .map_err(internal_join_error)
}

fn summarize_all(presets: &[arclain_core::SavedPreset]) -> Vec<PipelinePresetSummary> {
    let builtins = process::builtin_preset_summaries();
    presets
        .iter()
        .map(|preset| process::summarize_preset(preset, &builtins))
        .collect()
}

/// Writes the whole preset list back, on the blocking pool, and reports
/// the result as the caller's new view of it.
async fn store_presets(
    inner: &Arc<AppRuntime>,
    presets: Vec<arclain_core::SavedPreset>,
) -> Result<Vec<PipelinePresetSummary>, ApplicationError> {
    let path = inner.paths().presets_file();
    let write_path = path.clone();
    let written = handle_for(inner)?
        // Hands the list to the blocking task and takes it back rather
        // than cloning it alongside: `arclain_core::save_presets`
        // already copies internally to serialize.
        .spawn_blocking(move || arclain_core::save_presets(&write_path, &presets).map(|()| presets))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| presets_write_error(path, error))?;
    Ok(summarize_all(&written))
}

// ============================================================================
// Presets.
// ============================================================================

pub(super) async fn run_pipeline_presets(
    inner: &Arc<AppRuntime>,
) -> Result<Vec<PipelinePresetSummary>, ApplicationError> {
    Ok(summarize_all(&load_presets(inner).await?))
}

pub(super) async fn run_save_pipeline_preset(
    inner: &Arc<AppRuntime>,
    input: PipelinePresetInput,
) -> Result<Vec<PipelinePresetSummary>, ApplicationError> {
    // Structural validation first: a malformed preset never reaches the
    // write lock, let alone the file.
    let preset = process::preset_to_core(&input)?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let mut presets = load_presets(inner).await?;
    // Compared on the *trimmed* stored name, against an already-trimmed
    // new one. A presets file written before this facade existed can
    // hold `"  Padded  "`, which a frontend then shows -- and offers to
    // edit -- as `"Padded"`; an exact comparison would miss it and push
    // a second row the dropdown renders identically to the first, which
    // is precisely the duplicate this upsert exists to prevent.
    match presets
        .iter()
        .position(|candidate| candidate.name.trim() == preset.name)
    {
        // Replaced in place, so re-saving a preset does not shuffle it
        // to the bottom of the user's dropdown -- and a legacy padded
        // name is normalized by the same write.
        Some(index) => presets[index] = preset,
        None => presets.push(preset),
    }
    store_presets(inner, presets).await
}

/// Deletes by exact name.
///
/// Deliberately *not* trimmed, unlike the save path: a caller passes
/// back a name this surface listed, and a presets file written before
/// this facade existed can hold a name with surrounding whitespace.
/// Trimming here would make exactly those entries unlistable-away --
/// reported as present, then never matched by a delete.
pub(super) async fn run_delete_pipeline_preset(
    inner: &Arc<AppRuntime>,
    name: String,
) -> Result<Vec<PipelinePresetSummary>, ApplicationError> {
    let _write_guard = inner.settings_write_lock.lock().await;
    let mut presets = load_presets(inner).await?;
    let before = presets.len();
    presets.retain(|preset| preset.name != name);
    if presets.len() == before {
        return Err(preset_not_found_error(&name));
    }
    store_presets(inner, presets).await
}

// ============================================================================
// Preview.
// ============================================================================

pub(super) async fn run_preview_pipeline(
    inner: &Arc<AppRuntime>,
    request: PipelinePreviewRequest,
) -> Result<PipelinePreviewDto, ApplicationError> {
    // Resolved by the *same* function `start_pipeline` resolves its own
    // spec with, so a preview and the run it predicts can never turn one
    // spec into two different pipelines.
    let mut template =
        super::processing_ops::resolve_pipeline_spec(inner, &request.pipeline).await?;
    template.output = request.destination.to_core();
    if let Some(policy) = request.collision_policy {
        template.collision_policy = Some(policy.to_core());
    }

    let inputs = request.inputs.to_core();
    let services = inner.core_services().clone();

    // Everything below touches the filesystem (a stat per predicted
    // output, a directory read for a folder input) and the configuration
    // database, so it runs on the blocking pool rather than on an async
    // worker -- and it is awaited here, so nothing outlives this call.
    handle_for(inner)?
        .spawn_blocking(move || preview_blocking(template, inputs, &services))
        .await
        .map_err(internal_join_error)
}

/// Builds the preview: one `arclain_core` preview call **per input**,
/// each with that input's own resolved metadata.
///
/// # Why per input rather than one call for the batch
///
/// The predicted output *name* is a function of the metadata, and
/// `arclain_core::execute_pipeline` resolves that metadata separately
/// for every file it processes, from a product code detected in that
/// file's own name. A preview that resolved one blob for the whole batch
/// would predict the same name for every input -- which is not merely
/// imprecise, it is the wrong answer in a way that hides a collision:
/// N inputs shown as N identical output paths are N writes to one path.
/// Looping here mirrors `processing_ops::run_pipeline_over_inputs`,
/// which already calls `execute_pipeline` once per file for its own
/// reasons, so the two loops line up one-to-one.
fn preview_blocking(
    mut template: arclain_core::Pipeline,
    input: arclain_core::PipelineInput,
    services: &arclain_core::services::Services,
) -> PipelinePreviewDto {
    // The collision ladder, completed. `arclain_core`'s preview resolves
    // an unset policy to a hardcoded `Smart`, while its executor resolves
    // one to the user's `default_collision_policy` setting -- so a
    // profile that set `Overwrite` had the preview predict a failure the
    // run did not have. Materializing the setting here, from the same
    // read `processing_ops::build_pipeline_context` performs for the run,
    // makes both ends resolve the identical policy. Left `None` when the
    // setting is unset, which is exactly when the executor's own
    // `unwrap_or(Smart)` and the preview's hardcoded `Smart` agree.
    if template.collision_policy.is_none() {
        template.collision_policy = super::processing_ops::configured_collision_policy(services);
    }

    // A folder is expanded by one metadata-less pass through core: its
    // entry list *is* core's own expansion and its global warnings are
    // core's own words for an unreadable or archive-less directory, so
    // neither is re-derived here. A file list needs no such pass.
    let (paths, mut global_warnings) = match &input {
        arclain_core::PipelineInput::Files(paths) => (paths.clone(), None),
        arclain_core::PipelineInput::Folder(_) => {
            let mut scout = template.clone();
            scout.input = Some(input.clone());
            let scouted = arclain_core::preview_pipeline_with_metadata(&scout, None);
            (
                scouted
                    .entries
                    .into_iter()
                    .map(|entry| entry.input)
                    .collect(),
                Some(scouted.global_warnings),
            )
        }
    };

    let mut entries = Vec::with_capacity(paths.len());
    for path in &paths {
        let metadata = super::processing_ops::resolve_metadata(
            // The executor keys its lookup on the input's file name
            // (`executor.rs::run_one`), so this does too -- keying on
            // anything else would detect a different product code and
            // resolve different metadata.
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
            services.library_service.as_ref(),
        );
        let mut per_file = template.clone();
        per_file.input = Some(arclain_core::PipelineInput::Files(vec![path.clone()]));
        let mut preview =
            arclain_core::preview_pipeline_with_metadata(&per_file, metadata.as_ref());
        // Identical for every input (they depend on the step list alone),
        // so the first one answers for the batch.
        if global_warnings.is_none() {
            global_warnings = Some(std::mem::take(&mut preview.global_warnings));
        }
        entries.extend(
            preview
                .entries
                .into_iter()
                .map(process::preview_entry_to_dto),
        );
    }

    PipelinePreviewDto {
        entries,
        global_warnings: global_warnings.unwrap_or_else(|| {
            // An empty file list: core still has something to say about
            // the pipeline itself (an empty step list), and asking it
            // costs nothing when there is no file to stat.
            let mut probe = template;
            probe.input = Some(arclain_core::PipelineInput::Files(Vec::new()));
            arclain_core::preview_pipeline_with_metadata(&probe, None).global_warnings
        }),
    }
}

// ============================================================================
// Interrupted runs.
// ============================================================================

pub(super) async fn run_interrupted_pipeline_runs(
    inner: &Arc<AppRuntime>,
    since_unix: i64,
    limit: u32,
) -> Result<Vec<InterruptedPipelineRunDto>, ApplicationError> {
    // Empty (not an error) when no configuration database is open, the
    // same treatment the pre-facade page gave a missing one -- and the
    // same treatment `organization_ops` gives its own missing service.
    let Some(config_db) = inner.core_services().config_db.clone() else {
        return Ok(Vec::new());
    };

    // `SqliteDb::with_connection` takes a blocking mutex around the one
    // shared connection, so this must not run on an async worker.
    handle_for(inner)?
        .spawn_blocking(move || {
            config_db.with_connection(|conn| {
                Ok(arclain_core::list_interrupted_since(conn, since_unix)?
                    .into_iter()
                    // Newest first, so a bounded read keeps the rows a
                    // caller would actually show -- the underlying query
                    // orders by `completed_at DESC`.
                    .take(limit as usize)
                    .filter_map(|run| {
                        // The query filters `completed_at IS NOT NULL`,
                        // so this is unreachable; a row that somehow had
                        // none is dropped rather than reported with a
                        // fabricated timestamp.
                        run.completed_at
                            .map(|interrupted_at| InterruptedPipelineRunDto {
                                input_path: PathBuf::from(run.input_path),
                                started_at_unix: run.started_at,
                                interrupted_at_unix: interrupted_at,
                                arclain_version: run.arclain_version,
                            })
                    })
                    .collect::<Vec<_>>())
            })
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("listing interrupted pipeline runs", error))
}

// ============================================================================
// Error helpers.
// ============================================================================

fn preset_not_found_error(name: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such pipeline preset")
        .with_diagnostic(format!("no preset named {name:?}"))
        .with_recoverability(Recoverability::UserAction)
        .with_field("name")
}

/// A failed presets write is `Persistence`, not `Backend`: the failure is
/// the config directory being unwritable (permissions, a full disk),
/// which no amount of retrying fixes on its own -- unlike a database
/// call that lost a lock race.
fn presets_write_error(path: PathBuf, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Persistence,
        "failed to save pipeline presets",
    )
    .with_diagnostic(format!("{error:#}"))
    .with_recoverability(Recoverability::UserAction)
    .with_path(path)
}

fn backend_error(context: &'static str, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, "pipeline-run storage failed")
        .with_diagnostic(format!("{context}: {error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
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
