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
use crate::ids::ArchiveSessionId;
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
    match presets
        .iter()
        .position(|candidate| candidate.name == preset.name)
    {
        // Replaced in place, so re-saving a preset does not shuffle it
        // to the bottom of the user's dropdown.
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
    let metadata = resolve_preview_metadata(inner, request.metadata).await?;
    // Resolved by the *same* function `start_pipeline` resolves its own
    // spec with, so a preview and the run it predicts can never turn one
    // spec into two different pipelines.
    let mut pipeline =
        super::processing_ops::resolve_pipeline_spec(inner, &request.pipeline).await?;
    pipeline.input = Some(request.inputs.to_core());
    pipeline.output = request.destination.to_core();
    if let Some(policy) = request.collision_policy {
        pipeline.collision_policy = Some(policy.to_core());
    }

    // `arclain_core`'s preview stats every predicted output and, for a
    // folder input, reads the directory -- filesystem work, so it runs
    // on the blocking pool rather than on an async worker. Awaited here,
    // so nothing outlives this call.
    handle_for(inner)?
        .spawn_blocking(move || {
            process::preview_to_dto(arclain_core::preview_pipeline_with_metadata(
                &pipeline,
                metadata.as_ref(),
            ))
        })
        .await
        .map_err(internal_join_error)
}

/// The plugin-reported metadata of the session a preview names, read
/// through the same [`crate::organization::session_metadata_for_planning`]
/// every other planner in this crate reads it with.
///
/// An unknown session id is a `NotFound` rather than a silent fallback
/// to "no metadata": the metadata decides the predicted output *names*,
/// so downgrading a stale session id to `None` would quietly show a
/// caller a completely different set of paths than it asked about.
async fn resolve_preview_metadata(
    inner: &Arc<AppRuntime>,
    session_id: Option<ArchiveSessionId>,
) -> Result<Option<arclain_core::GameMetadata>, ApplicationError> {
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let session = inner.archive_sessions().get(session_id).await?;
    Ok(crate::organization::session_metadata_for_planning(
        session.metadata(),
    ))
}

// ============================================================================
// Interrupted runs.
// ============================================================================

pub(super) async fn run_interrupted_pipeline_runs(
    inner: &Arc<AppRuntime>,
    since_unix: i64,
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
