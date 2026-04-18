//! Blocking pipeline executor — runs a `Pipeline` against each input file.

use super::context::PipelineContext;
use super::hashing::hash_file_blake3;
use super::types::{
    OutputCollisionPolicy, Pipeline, PipelineInput, PipelineStep,
};
use anyhow::{Context, Result};
use arclain_db::{pipeline_output_kind, NewPipelineRun};
use std::path::{Path, PathBuf};

const ARCLAIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Progress event emitted by the executor.
#[derive(Debug, Clone)]
pub enum PipelineProgress {
    FileStart {
        index: usize,
        total: usize,
        name: String,
    },
    StepStart {
        step_index: usize,
        step_name: String,
    },
    StepProgress {
        percent: u8,
    },
    FileComplete {
        output: PathBuf,
    },
    /// Input skipped because the pipeline's collision/dedup gate matched an
    /// existing output. `reason` describes why (e.g. "already processed",
    /// "output exists, Skip policy"). Distinct from `FileComplete` so callers
    /// can report processed vs skipped counts separately.
    FileSkipped {
        output: PathBuf,
        reason: String,
    },
    FileFailed {
        error: String,
    },
    AllComplete {
        succeeded: usize,
        skipped: usize,
        failed: usize,
    },
}

/// Result of a single-input pipeline run.
enum RunOutcome {
    Completed(PathBuf),
    Skipped { output: PathBuf, reason: String },
}

/// Execute a pipeline. Blocks until all inputs are processed.
pub fn execute_pipeline(
    pipeline: &Pipeline,
    temp_root: &Path,
    ctx: &PipelineContext,
    mut on_progress: impl FnMut(PipelineProgress),
) -> Result<()> {
    let inputs = resolve_inputs(&pipeline.input)?;
    let total = inputs.len();
    let mut succeeded = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for (idx, input) in inputs.iter().enumerate() {
        let name = input
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();

        on_progress(PipelineProgress::FileStart {
            index: idx,
            total,
            name: name.clone(),
        });

        match run_one(input, pipeline, temp_root, ctx, &mut on_progress) {
            Ok(RunOutcome::Completed(output)) => {
                succeeded += 1;
                on_progress(PipelineProgress::FileComplete { output });
            }
            Ok(RunOutcome::Skipped { output, reason }) => {
                skipped += 1;
                on_progress(PipelineProgress::FileSkipped { output, reason });
            }
            Err(e) => {
                failed += 1;
                on_progress(PipelineProgress::FileFailed {
                    error: e.to_string(),
                });
            }
        }
    }

    on_progress(PipelineProgress::AllComplete {
        succeeded,
        skipped,
        failed,
    });
    Ok(())
}

/// Walk the step list for the last Convert step's format so we can predict
/// the final output extension before extraction begins.
fn last_convert_format(steps: &[PipelineStep]) -> Option<crate::features::conversion::ConvertFormat> {
    for step in steps.iter().rev() {
        if let PipelineStep::Convert { format, .. } = step {
            return Some(format.clone());
        }
    }
    None
}

fn resolve_inputs(input: &Option<PipelineInput>) -> Result<Vec<PathBuf>> {
    match input {
        None => anyhow::bail!("No input provided"),
        Some(PipelineInput::Files(v)) => Ok(v.clone()),
        Some(PipelineInput::Folder(p)) => {
            crate::features::conversion::flatten::find_archive_files(p)
        }
    }
}

fn run_one(
    input: &Path,
    pipeline: &Pipeline,
    temp_root: &Path,
    ctx: &PipelineContext,
    on_progress: &mut impl FnMut(PipelineProgress),
) -> Result<RunOutcome> {
    // Hash the input and pipeline config up-front. These are the dedup key
    // for `pipeline_runs` lookups and also form the basis for Smart-mode
    // "we already did this exact work" detection.
    let (input_blake3, input_size) = hash_file_blake3(input)
        .with_context(|| format!("hash input {:?}", input))?;
    let pipeline_hash = pipeline.config_hash();

    // Predict the final archive path from the pipeline's last Convert step
    // (or the implicit zip fallback).
    let final_format = last_convert_format(&pipeline.steps)
        .unwrap_or(crate::features::conversion::ConvertFormat::Zip);
    let predicted_output_path = pipeline.output.resolve(input, final_format.extension());

    // Phase 3: Smart consults the DB. If we already completed this exact
    // input+pipeline AND the output is still on disk, skip. No matching row
    // OR missing output → treat as a fresh run.
    let app_default = ctx
        .default_collision_policy
        .unwrap_or(OutputCollisionPolicy::Smart);
    let policy = pipeline.effective_collision_policy(app_default);
    let path_exists = predicted_output_path.exists();

    if matches!(policy, OutputCollisionPolicy::Smart) {
        if let Some(db) = ctx.config_db.as_ref() {
            let completed = db
                .with_connection(|conn| {
                    Ok(arclain_db::find_completed_run(
                        conn,
                        &input_blake3,
                        &pipeline_hash,
                    )?)
                })
                .ok()
                .flatten();

            if let Some(run) = completed {
                if let Some(ref stored) = run.output_path {
                    if std::path::Path::new(stored).exists() {
                        tracing::info!(
                            "[pipeline] Smart skip: {} already produced {} (run #{})",
                            input.display(),
                            stored,
                            run.id
                        );
                        return Ok(RunOutcome::Skipped {
                            output: PathBuf::from(stored),
                            reason: "already processed".to_string(),
                        });
                    }
                }
            }
        }
    }

    if path_exists {
        match policy {
            OutputCollisionPolicy::Skip => {
                tracing::info!(
                    "[pipeline] Skipping {}: output exists at {} (policy=Skip)",
                    input.display(),
                    predicted_output_path.display()
                );
                return Ok(RunOutcome::Skipped {
                    output: predicted_output_path,
                    reason: "output exists (Skip policy)".to_string(),
                });
            }
            OutputCollisionPolicy::Overwrite => {
                tracing::info!(
                    "[pipeline] Overwriting {} (policy=Overwrite)",
                    predicted_output_path.display()
                );
                if predicted_output_path.is_dir() {
                    std::fs::remove_dir_all(&predicted_output_path).with_context(|| {
                        format!("Remove existing folder {:?}", predicted_output_path)
                    })?;
                } else {
                    std::fs::remove_file(&predicted_output_path).with_context(|| {
                        format!("Remove existing file {:?}", predicted_output_path)
                    })?;
                }
            }
            OutputCollisionPolicy::Fail | OutputCollisionPolicy::Smart => {
                // Smart with no matching DB row: output exists but we can't
                // prove we produced it. Refuse rather than silently overwriting
                // a file that may belong to someone else.
                anyhow::bail!(
                    "Output already exists at {} (collision policy: {}). \
                     Arclain has no record of producing it — set policy to \
                     Overwrite or Skip to proceed.",
                    predicted_output_path.display(),
                    policy.display_name()
                );
            }
        }
    }

    // Record the run as in_progress before doing any work. DB failures are
    // logged but don't abort — we'd rather lose audit data than the work.
    let input_path_str = input.to_string_lossy().into_owned();
    let run_id = ctx.config_db.as_ref().and_then(|db| {
        db.with_connection(|conn| {
            let new_run = NewPipelineRun {
                input_path: &input_path_str,
                input_blake3: &input_blake3,
                input_size: input_size as i64,
                pipeline_hash: &pipeline_hash,
                arclain_version: ARCLAIN_VERSION,
            };
            Ok(arclain_db::begin_pipeline_run(conn, &new_run)?)
        })
        .map_err(|e| {
            tracing::warn!("[pipeline] Failed to record in_progress run: {}", e);
            e
        })
        .ok()
    });

    let _ = final_format; // predicted format was used for the collision gate only
    let run_result = run_one_inner(input, pipeline, temp_root, ctx, on_progress);

    // Record completion/failure. Same logic: log and continue on DB errors.
    if let (Some(id), Some(db)) = (run_id, ctx.config_db.as_ref()) {
        match &run_result {
            Ok(output_path) => {
                let kind = if output_path.is_dir() {
                    pipeline_output_kind::FOLDER
                } else {
                    pipeline_output_kind::ARCHIVE
                };
                let output_str = output_path.to_string_lossy().to_string();
                if let Err(e) = db.with_connection(|conn| {
                    Ok(arclain_db::mark_run_completed(conn, id, &output_str, kind)?)
                }) {
                    tracing::warn!("[pipeline] Failed to mark run #{} completed: {}", id, e);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if let Err(e2) = db.with_connection(|conn| {
                    Ok(arclain_db::mark_run_failed(conn, id, &msg)?)
                }) {
                    tracing::warn!("[pipeline] Failed to mark run #{} failed: {}", id, e2);
                }
            }
        }
    }

    run_result.map(RunOutcome::Completed)
}

/// The body of `run_one` — extraction + steps + final pack. Split out so the
/// outer `run_one` can own the DB bookkeeping around it.
fn run_one_inner(
    input: &Path,
    pipeline: &Pipeline,
    temp_root: &Path,
    ctx: &PipelineContext,
    on_progress: &mut impl FnMut(PipelineProgress),
) -> Result<PathBuf> {
    let work_dir = temp_root.join(format!(
        "arclain_pipeline_{}_{}",
        std::process::id(),
        input
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("x")
    ));
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("Create work dir {:?}", work_dir))?;
    let _cleanup = WorkDirGuard(work_dir.clone());

    // Extract source
    on_progress(PipelineProgress::StepStart {
        step_index: 0,
        step_name: "Extracting source".to_string(),
    });
    let source_backend = (ctx.backend_for)(input)?;
    source_backend
        .extract_all(input, &work_dir, None)
        .with_context(|| format!("Extract {:?}", input))?;

    let mut final_format: Option<crate::features::conversion::ConvertFormat> = None;
    let mut final_compression = crate::features::conversion::CompressionLevel::Normal;

    for (step_idx, step) in pipeline.steps.iter().enumerate() {
        on_progress(PipelineProgress::StepStart {
            step_index: step_idx + 1,
            step_name: step.display_name().to_string(),
        });

        match step {
            PipelineStep::Flatten {
                strip_common_prefix,
                max_depth,
            } => {
                crate::features::conversion::flatten::flatten_nested_archives_recursive(
                    &work_dir,
                    *strip_common_prefix,
                    *max_depth,
                    |archive_path, dest_dir| {
                        let be = (ctx.backend_for)(archive_path)?;
                        be.extract_all(archive_path, dest_dir, None)
                    },
                )?;
            }
            PipelineStep::Organize { rule_id } => {
                use crate::features::organization::engine::RuleEngine;

                let org_svc = ctx
                    .organization_service
                    .as_ref()
                    .context("Organize step requires OrganizationService")?;

                let rule = org_svc
                    .get_domain_rule(*rule_id)
                    .context("Failed to load rule")?
                    .ok_or_else(|| anyhow::anyhow!("Rule #{} not found", rule_id))?;

                let entries = scan_work_dir_as_entries(&work_dir)?;

                let archive_name = input
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let metadata = resolve_metadata(&archive_name, ctx);

                let plan = RuleEngine::create_plan(
                    &rule,
                    &archive_name,
                    &entries,
                    metadata.as_ref(),
                )
                .context("Rule plan failed")?;

                crate::features::pipeline::apply_plan::apply_plan_to_workdir(
                    &plan, &work_dir,
                )
                .context("Apply plan failed")?;
            }
            PipelineStep::Convert {
                format,
                compression,
                password: _,
            } => {
                final_format = Some(format.clone());
                final_compression = *compression;
            }
        }
    }

    // If no Convert step, default to zip so we always produce output
    let format = final_format.unwrap_or(crate::features::conversion::ConvertFormat::Zip);
    let output_path = pipeline.output.resolve(input, format.extension());

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let cli = crate::backends::SevenZipCli::detect(None)
        .context("7z CLI not found — required for pipeline conversion")?;
    let handle = cli
        .spawn_convert_with_progress(&work_dir, &output_path, format, final_compression)
        .context("Failed to spawn 7z compression")?;

    drain_progress(handle, on_progress)?;

    Ok(output_path)
}

fn drain_progress(
    mut handle: crate::backends::sevenz_cli::ChildWithProgress,
    on_progress: &mut impl FnMut(PipelineProgress),
) -> Result<()> {
    // The sender is dropped when the 7z process exits, so `recv()` eventually
    // returns Err, letting us exit the loop.
    loop {
        match handle.rx.recv() {
            Ok(update) => {
                on_progress(PipelineProgress::StepProgress {
                    percent: update.percent,
                });
            }
            Err(_) => break,
        }
    }

    // Collect process exit status
    let status = handle.child.wait().context("Waiting for 7z process")?;
    if !status.success() {
        anyhow::bail!("7z exited with non-success status: {:?}", status);
    }
    Ok(())
}

struct WorkDirGuard(PathBuf);
impl Drop for WorkDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Recursively walk `dir`, producing ArchiveEntry records with paths
/// relative to `dir` using forward slashes.
fn scan_work_dir_as_entries(dir: &Path) -> Result<Vec<crate::archive::ArchiveEntry>> {
    let mut entries = Vec::new();
    walk_collect(dir, dir, &mut entries)?;
    Ok(entries)
}

fn walk_collect(
    root: &Path,
    current: &Path,
    out: &mut Vec<crate::archive::ArchiveEntry>,
) -> Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let is_dir = path.is_dir();
        let size = if is_dir {
            0
        } else {
            path.metadata().map(|m| m.len()).unwrap_or(0)
        };
        out.push(crate::archive::ArchiveEntry {
            path: rel,
            size,
            packed_size: size,
            is_dir,
            encrypted: false,
            modified: None,
            crc32: None,
        });
        if is_dir {
            walk_collect(root, &path, out)?;
        }
    }
    Ok(())
}

/// Attempt to find metadata for the archive via LibraryService.
/// For 1.3 we only try DLSite lookup by filename pattern.
fn resolve_metadata(
    archive_name: &str,
    ctx: &PipelineContext,
) -> Option<crate::features::organization::metadata::GameMetadata> {
    let lib = ctx.library_service.as_ref()?;
    let code = crate::utilities::detect_dlsite_code(archive_name)?;
    let id = format!("dlsite:{}", code);
    let product = lib.get_metadata(&id).ok().flatten()?;
    let json = serde_json::to_string(&product).ok()?;
    crate::features::organization::metadata::GameMetadata::from_json(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_rejects_no_input() {
        let p = Pipeline::default();
        let tmp = tempfile::tempdir().unwrap();
        let ctx = PipelineContext::minimal(|_| anyhow::bail!("no backend"));
        let result = execute_pipeline(&p, tmp.path(), &ctx, |_| {});
        assert!(result.is_err());
    }
}
