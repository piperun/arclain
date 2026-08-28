//! Blocking pipeline executor — runs a `Pipeline` against each input file.

use super::context::PipelineContext;
use super::hashing::hash_file_blake3_with_progress;
use super::output_transaction::StagedOutput;
use super::types::{OutputArtifact, OutputCollisionPolicy, Pipeline, PipelineInput, PipelineStep};
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
    /// Read-only diagnostic warnings emitted by a step (currently
    /// only Flatten). Surfaces source-archive quality issues such as
    /// missing screenshot files or orphan addons. Step succeeded; this
    /// is informational only.
    StepWarnings {
        warnings: Vec<crate::features::conversion::ModWarning>,
    },
    /// One line per download an `Organize` step's plan scheduled but
    /// could not resolve — an unreachable host, a refused request, a
    /// context with no transport configured. Each names the destination
    /// that stayed empty as well as the URL, because the reader wants to
    /// know which screenshot is missing, not only which fetch failed.
    /// The step succeeded and the rest of the plan was applied; this is
    /// informational only.
    ///
    /// Distinct from `StepWarnings`, which carries the structured
    /// `ModWarning` diagnostics Flatten produces about a source
    /// archive's own contents. A download that did not arrive is a
    /// property of this run, not of the archive.
    DownloadWarnings {
        warnings: Vec<String>,
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
fn last_convert_format(
    steps: &[PipelineStep],
) -> Option<crate::features::conversion::ConvertFormat> {
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
    //
    // Hashing a multi-GB archive is the first visible delay in the pipeline,
    // so emit a dedicated step + progress events so the UI doesn't look frozen.
    on_progress(PipelineProgress::StepStart {
        step_index: 0,
        step_name: "Hashing input".to_string(),
    });
    let (input_blake3, input_size) = hash_file_blake3_with_progress(input, |percent| {
        on_progress(PipelineProgress::StepProgress { percent });
    })
    .with_context(|| format!("hash input {:?}", input))?;
    let pipeline_hash = pipeline.config_hash();

    // Resolve metadata for this input ONCE (DB lookup) and reuse it for both
    // the predicted output path AND the finalize step, so the path the
    // collision check examines matches the path the executor actually writes
    // to. Without this both paths could disagree on whether to use the
    // metadata title vs the input stem.
    let archive_name_for_meta = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let output_metadata = resolve_metadata(&archive_name_for_meta, ctx);

    // Predict the final output path. For Archive mode we use the last Convert
    // step's extension (or zip as the fallback). For Folder mode the artifact
    // is a directory named after the input's stem (or metadata title).
    let final_format = last_convert_format(&pipeline.steps)
        .unwrap_or(crate::features::conversion::ConvertFormat::Zip);
    let predicted_output_path = match pipeline.output_artifact {
        OutputArtifact::Archive => pipeline.output.resolve_with_metadata(
            input,
            final_format.extension(),
            output_metadata.as_ref(),
        ),
        OutputArtifact::Folder => pipeline
            .output
            .resolve_folder_with_metadata(input, output_metadata.as_ref()),
    };

    // Phase 3: Smart consults the DB. If we already completed this exact
    // input+pipeline AND the output is still on disk, skip. No matching row
    // OR missing output → treat as a fresh run.
    //
    // This preliminary check supports early Skip/Fail feedback. Commit is the
    // authoritative collision gate: it rechecks under a destination-scoped
    // process lock and promotes with an atomic no-replace rename, so a
    // concurrent arrival cannot be overwritten after this check.
    let app_default = ctx
        .default_collision_policy
        .unwrap_or(OutputCollisionPolicy::Smart);
    let policy = pipeline.effective_collision_policy(app_default);
    let replace_existing = matches!(policy, OutputCollisionPolicy::Overwrite);
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
    let run_result = run_one_inner(
        input,
        pipeline,
        temp_root,
        ctx,
        on_progress,
        output_metadata.as_ref(),
        replace_existing,
    );

    // Record completion/failure. Same logic: log and continue on DB errors.
    if let (Some(id), Some(db)) = (run_id, ctx.config_db.as_ref()) {
        match &run_result {
            Ok(output_path) => {
                let kind = if output_path.is_dir() {
                    pipeline_output_kind::FOLDER
                } else {
                    pipeline_output_kind::ARCHIVE
                };
                let output_str = output_path.to_string_lossy().into_owned();
                if let Err(e) = db.with_connection(|conn| {
                    Ok(arclain_db::mark_run_completed(conn, id, &output_str, kind)?)
                }) {
                    tracing::warn!("[pipeline] Failed to mark run #{} completed: {}", id, e);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if let Err(e2) =
                    db.with_connection(|conn| Ok(arclain_db::mark_run_failed(conn, id, &msg)?))
                {
                    tracing::warn!("[pipeline] Failed to mark run #{} failed: {}", id, e2);
                }
            }
        }
    }

    run_result.map(RunOutcome::Completed)
}

/// Resolve the downloads `plan` schedules into files under `work_dir`,
/// returning the plan the applier should run and one warning line per
/// download that did not arrive.
///
/// The returned plan carries each fetched image as an ordinary move, so
/// the applier places and rolls back a screenshot with everything else
/// and performs no network I/O of its own.
///
/// A download that fails is reported, not fatal: a missing screenshot
/// must not cost the user the reorganization it came with. A context
/// with no `fetch_download` therefore resolves nothing and reports every
/// scheduled download rather than failing the step.
fn stage_plan_downloads_for(
    plan: &crate::features::organization::engine::OrganizationPlan,
    work_dir: &Path,
    ctx: &PipelineContext,
) -> Result<(
    crate::features::organization::engine::OrganizationPlan,
    Vec<String>,
)> {
    let staged = crate::features::organization::downloads::stage_plan_downloads(
        plan,
        work_dir,
        &|download| match ctx.fetch_download.as_ref() {
            Some(fetch) => fetch(download),
            None => anyhow::bail!("no download transport configured"),
        },
    )?;

    let warnings = staged
        .unfetched
        .iter()
        .map(|(url, dest_path, reason)| {
            format!("screenshot not fetched: {dest_path} ({url}): {reason}")
        })
        .collect();

    Ok((staged.plan, warnings))
}

/// The body of `run_one` — extraction + steps + final pack. Split out so the
/// outer `run_one` can own the DB bookkeeping around it.
fn run_one_inner(
    input: &Path,
    pipeline: &Pipeline,
    temp_root: &Path,
    ctx: &PipelineContext,
    on_progress: &mut impl FnMut(PipelineProgress),
    output_metadata: Option<&crate::features::organization::metadata::GameMetadata>,
    replace_existing: bool,
) -> Result<PathBuf> {
    let archive_name_for_meta = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let work_dir = temp_root.join(format!(
        "arclain_pipeline_{}_{}",
        std::process::id(),
        input.file_name().and_then(|n| n.to_str()).unwrap_or("x")
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
                let report =
                    crate::features::conversion::flatten::flatten_nested_archives_recursive(
                        &work_dir,
                        *strip_common_prefix,
                        *max_depth,
                        |archive_path, dest_dir| {
                            let be = (ctx.backend_for)(archive_path)?;
                            be.extract_all(archive_path, dest_dir, None)
                        },
                    )?;
                if !report.warnings.is_empty() {
                    on_progress(PipelineProgress::StepWarnings {
                        warnings: report.warnings,
                    });
                }
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

                // A pipeline plans after extracting, so a layout's file
                // variables are read out of the work directory. Routed
                // through `CheckedRelativePath` so a layout cannot name
                // a path that climbs out of it or follows a symlink.
                let read_entry = |path: &str| -> Option<Vec<u8>> {
                    let checked = crate::utilities::CheckedRelativePath::new(path).ok()?;
                    std::fs::read(checked.resolve_under(&work_dir).ok()?).ok()
                };

                // Reuse the metadata resolved at the top of run_one so we
                // don't repeat the DB lookup for each Organize step.
                let plan = RuleEngine::create_plan(
                    &rule,
                    archive_name_for_meta,
                    &entries,
                    output_metadata,
                    &read_entry,
                )
                .context("Rule plan failed")?;

                // Resolve the plan's scheduled images to files on disk
                // before the applier runs, so the applier's transaction
                // over the work directory never waits on the network.
                let (plan, download_warnings) = stage_plan_downloads_for(&plan, &work_dir, ctx)
                    .context("Staging plan downloads failed")?;
                if !download_warnings.is_empty() {
                    on_progress(PipelineProgress::DownloadWarnings {
                        warnings: download_warnings,
                    });
                }

                crate::features::pipeline::apply_plan::apply_plan_to_workdir(&plan, &work_dir)
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

    // Finalize. Two paths:
    //   Archive → pack work_dir via 7z CLI (historical behavior)
    //   Folder  → move work_dir to the output location, no repacking
    //
    // Both reuse `output_metadata` resolved at the start of run_one so the
    // final write path matches `predicted_output_path` from the collision
    // check above.
    match pipeline.output_artifact {
        OutputArtifact::Archive => {
            let format = final_format.unwrap_or(crate::features::conversion::ConvertFormat::Zip);
            let output_path =
                pipeline
                    .output
                    .resolve_with_metadata(input, format.extension(), output_metadata);

            on_progress(PipelineProgress::StepStart {
                step_index: pipeline.steps.len() + 1,
                step_name: format!("Packing .{}", format.extension()),
            });

            let staged = StagedOutput::new(&output_path, replace_existing)?;
            let cli = crate::backends::SevenZipCli::detect(None)
                .context("7z CLI not found — required for pipeline conversion")?;
            let handle = cli
                .spawn_convert_with_progress(
                    &work_dir,
                    staged.artifact_path(),
                    format,
                    final_compression,
                )
                .context("Failed to spawn 7z compression")?;
            drain_progress(handle, on_progress)?;
            staged.verify(OutputArtifact::Archive)?;
            staged.commit()
        }
        OutputArtifact::Folder => {
            let output_path = pipeline
                .output
                .resolve_folder_with_metadata(input, output_metadata);

            on_progress(PipelineProgress::StepStart {
                step_index: pipeline.steps.len() + 1,
                step_name: "Writing folder output".to_string(),
            });

            // Copy the complete work tree to a destination-sibling stage.
            // The source remains intact until the staged tree is verified and
            // atomically promoted.
            let staged = StagedOutput::new(&output_path, replace_existing)?;
            copy_dir_recursive(&work_dir, staged.artifact_path()).with_context(|| {
                format!(
                    "staging folder output from {} beside {}",
                    work_dir.display(),
                    output_path.display()
                )
            })?;
            staged.verify(OutputArtifact::Folder)?;
            let output_path = staged.commit()?;
            on_progress(PipelineProgress::StepProgress { percent: 100 });

            Ok(output_path)
        }
    }
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir(dest).with_context(|| format!("Creating {:?}", dest))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("Reading {:?}", src))? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("Reading filesystem type for {:?}", from))?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to).with_context(|| format!("Copying {:?} to {:?}", from, to))?;
        } else {
            anyhow::bail!(
                "Folder output contains a symlink or special filesystem node: {}",
                from.display()
            );
        }
    }
    Ok(())
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
///
/// Serializes the looked-up `gameta_core::ProductMetadata` via
/// `to_plugin_json_string()`, not its own plain `#[derive(Serialize)]`
/// (which emits a flat shape keyed `external_id`): `GameMetadata::
/// from_json` expects `to_plugin_json()`'s *layered* shape instead
/// (`product_id` at the top level, nested `common`/`<source>` objects),
/// and `GameMetadata::product_id` has no `#[serde(default)]`, so the
/// wrong shape made deserialization fail outright every time -- silently,
/// since the `.ok()` this function already had swallowed the error --
/// and this function always returned `None` for any archive with real,
/// matched metadata as a result. See
/// `resolve_metadata_recovers_the_seeded_title_not_just_the_detected_product_code`
/// below for the regression proof.
#[cfg(feature = "gameta")]
fn resolve_metadata(
    archive_name: &str,
    ctx: &PipelineContext,
) -> Option<crate::features::organization::metadata::GameMetadata> {
    let lib = ctx.library_service.as_ref()?;
    let code = crate::utilities::detect_dlsite_code(archive_name)?;
    let id = format!("dlsite:{}", code);
    let product = lib.get_metadata(&id).ok().flatten()?;
    crate::features::organization::metadata::GameMetadata::from_json(
        &product.to_plugin_json_string(),
    )
    .ok()
}

/// Metadata resolution requires the `gameta` feature; without it every
/// archive resolves no metadata and naming falls through to the
/// detected-code/file-stem tiers.
#[cfg(not(feature = "gameta"))]
fn resolve_metadata(
    _archive_name: &str,
    _ctx: &PipelineContext,
) -> Option<crate::features::organization::metadata::GameMetadata> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan scheduling one screenshot, shaped as `RuleEngine::create_plan`
    /// emits it: the image is a `PendingDownload`, not yet a move.
    fn plan_scheduling_one_screenshot() -> crate::features::organization::engine::OrganizationPlan {
        use crate::features::organization::engine::{OrganizationPlan, PendingDownload};

        use crate::features::organization::engine::PlannedOutput;

        OrganizationPlan {
            rule_name: "Test".to_string(),
            outputs: vec![PlannedOutput {
                root_folder: "Root".to_string(),
                root_folder_template: "Root".to_string(),
                moves: vec![("a.txt".to_string(), "Root/a.txt".to_string())],
                generated_files: vec![],
                downloads: vec![PendingDownload {
                    product_id: Some("RJ123456".to_string()),
                    url: "https://img.example.test/RJ123456_img_main.jpg".to_string(),
                    dest_path: "Root/screenshots/image_001.jpg".to_string(),
                    cache_key: "dlsite:RJ123456:screenshot_0".to_string(),
                    cached: false,
                }],
                resolved_variables: Default::default(),
                reasoning: vec![],
            }],
            skipped_outputs: vec![],
        }
    }

    /// The pipeline must stay usable with no transport composed — a lean
    /// build, or a context assembled without network access. The
    /// reorganization still lands; only the images are missing, and each
    /// one is named so the user can see which.
    #[test]
    fn a_context_with_no_transport_still_applies_the_plan_and_names_each_missing_image() {
        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("a.txt"), b"x").unwrap();

        let ctx = PipelineContext::minimal(|_| anyhow::bail!("no backend"));
        assert!(
            ctx.fetch_download.is_none(),
            "a minimal context must compose no transport"
        );

        let (plan, warnings) =
            stage_plan_downloads_for(&plan_scheduling_one_screenshot(), work.path(), &ctx)
                .expect("a missing transport must not fail the step");

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("Root/screenshots/image_001.jpg"),
            "the warning must name the destination that stayed empty: {}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("https://img.example.test/RJ123456_img_main.jpg"),
            "the warning must name the URL: {}",
            warnings[0]
        );

        crate::features::pipeline::apply_plan::apply_plan_to_workdir(&plan, work.path())
            .expect("the rest of the plan must still apply");
        assert!(
            work.path().join("Root/a.txt").exists(),
            "an unreachable screenshot must not cost the user the reorganization"
        );
        assert!(!work.path().join("Root/screenshots/image_001.jpg").exists());
    }

    /// One screenshot failing must not drop the ones that did arrive,
    /// and the survivor must still be the image its plan named.
    #[test]
    fn a_failed_screenshot_is_reported_while_its_siblings_still_land() {
        use crate::features::organization::engine::PendingDownload;

        let work = tempfile::tempdir().unwrap();
        std::fs::write(work.path().join("a.txt"), b"x").unwrap();

        let mut plan = plan_scheduling_one_screenshot();
        plan.outputs[0].downloads.push(PendingDownload {
            product_id: Some("RJ123456".to_string()),
            url: "https://img.example.test/RJ123456_img_smp1.jpg".to_string(),
            dest_path: "Root/screenshots/image_002.jpg".to_string(),
            cache_key: "dlsite:RJ123456:screenshot_1".to_string(),
            cached: false,
        });

        let ctx = PipelineContext {
            fetch_download: Some(std::sync::Arc::new(|download: &PendingDownload| {
                if download.dest_path.ends_with("image_002.jpg") {
                    anyhow::bail!("status 404");
                }
                Ok(b"jpegbytes".to_vec())
            })),
            ..PipelineContext::minimal(|_| anyhow::bail!("no backend"))
        };

        let (plan, warnings) = stage_plan_downloads_for(&plan, work.path(), &ctx).expect("staging");

        assert_eq!(warnings.len(), 1, "only the failed image is reported");
        assert!(warnings[0].contains("image_002.jpg"));
        assert!(warnings[0].contains("status 404"));

        crate::features::pipeline::apply_plan::apply_plan_to_workdir(&plan, work.path())
            .expect("apply");
        assert!(
            work.path().join("Root/screenshots/image_001.jpg").exists(),
            "the screenshot that arrived must still be placed"
        );
        assert!(!work.path().join("Root/screenshots/image_002.jpg").exists());
    }

    #[test]
    fn executor_rejects_no_input() {
        let p = Pipeline::default();
        let tmp = tempfile::tempdir().unwrap();
        let ctx = PipelineContext::minimal(|_| anyhow::bail!("no backend"));
        let result = execute_pipeline(&p, tmp.path(), &ctx, |_| {});
        assert!(result.is_err());
    }

    /// Regression: `resolve_metadata` serializes the looked-up
    /// `gameta_core::ProductMetadata` via its own plain
    /// `#[derive(Serialize)]` -- a flat shape keyed `external_id` -- and
    /// feeds the result to `GameMetadata::from_json`, whose own doc
    /// comment and test fixtures both show it expects
    /// `ProductMetadata::to_plugin_json()`'s *layered* shape instead
    /// (`product_id` at the top level, nested `common`/`<source>`
    /// objects). `GameMetadata::product_id` has no `#[serde(default)]`,
    /// so that mismatch makes the deserialization fail outright every
    /// time, and the `.ok()` on the line before it silently swallows the
    /// error -- `resolve_metadata` has therefore always returned `None`
    /// for *any* archive with real, matched metadata, regardless of
    /// title, falling back to the bare detected product code instead.
    /// Seeds a real `LibraryService` row (this crate's own established
    /// anonymized-fixture convention, `RJ123456`) and asserts both that
    /// `resolve_metadata` resolves the seeded title, and that the actual
    /// naming outcome (`PipelineOutput::resolve_with_metadata`) is
    /// title-based, not the raw detected code.
    #[cfg(feature = "gameta")]
    #[test]
    fn resolve_metadata_recovers_the_seeded_title_not_just_the_detected_product_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("metadata.sqlite");
        let library_service = crate::services::LibraryService::new(&db_path)
            .expect("constructing a fresh LibraryService must succeed");
        let mut metadata =
            gameta_core::ProductMetadata::new(gameta_core::MetadataSource::DLSite, "RJ123456");
        metadata.title = Some("Placeholder Test Title".to_string());
        library_service
            .save_metadata(&metadata)
            .expect("seeding test metadata must succeed");

        let ctx = PipelineContext {
            library_service: Some(std::sync::Arc::new(library_service)),
            ..PipelineContext::minimal(|_| anyhow::bail!("no backend"))
        };

        let resolved = resolve_metadata("[RJ123456] Placeholder Game.zip", &ctx).expect(
            "metadata must resolve for an archive whose name carries the seeded product code",
        );
        assert_eq!(resolved.title, "Placeholder Test Title");

        let input = Path::new("[RJ123456] Placeholder Game.zip");
        let output =
            crate::PipelineOutput::SameFolder.resolve_with_metadata(input, "zip", Some(&resolved));
        assert_eq!(
            output.file_name().and_then(|n| n.to_str()),
            Some("Placeholder Test Title.zip"),
            "the naming outcome must use the seeded title, not the bare detected product code"
        );
    }

    /// The same contract against the metadata a real library holds.
    /// gameta stores screenshots in `extras` as a list of source URLs,
    /// and `to_plugin_json` merges every `extras` key to the top level,
    /// so every product that has screenshots reaches
    /// `GameMetadata::from_json` carrying that list. One entry that
    /// fails to deserialize fails the whole document, and the `.ok()`
    /// above turns that into `None` -- losing the title, the creator and
    /// the tags, not just the screenshots. Seeded here with screenshots
    /// present, because the seed without them cannot see the failure.
    #[cfg(feature = "gameta")]
    #[test]
    fn resolve_metadata_survives_the_screenshot_urls_a_real_product_carries() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("metadata.sqlite");
        let library_service = crate::services::LibraryService::new(&db_path)
            .expect("constructing a fresh LibraryService must succeed");
        let mut metadata =
            gameta_core::ProductMetadata::new(gameta_core::MetadataSource::DLSite, "RJ123456");
        metadata.title = Some("Placeholder Test Title".to_string());
        metadata.extras = serde_json::json!({
            "screenshots": [
                "https://img.example.test/RJ123456_img_main.jpg",
                "https://img.example.test/RJ123456_img_smp1.jpg"
            ]
        });
        library_service
            .save_metadata(&metadata)
            .expect("seeding test metadata must succeed");

        let ctx = PipelineContext {
            library_service: Some(std::sync::Arc::new(library_service)),
            ..PipelineContext::minimal(|_| anyhow::bail!("no backend"))
        };

        let resolved = resolve_metadata("[RJ123456] Placeholder Game.zip", &ctx)
            .expect("metadata carrying screenshot URLs must still resolve");
        assert_eq!(resolved.title, "Placeholder Test Title");
        assert_eq!(resolved.screenshots.len(), 2);
    }

    /// The other half of that contract: without the `gameta` feature
    /// there is no store to look the code up in, so the same archive
    /// name resolves nothing at all and naming falls through to the
    /// file-stem tier. Asserted here so a lean build that silently
    /// grew a metadata path would fail rather than pass unnoticed.
    #[cfg(not(feature = "gameta"))]
    #[test]
    fn resolve_metadata_without_gameta_resolves_nothing_for_a_coded_archive() {
        let ctx = PipelineContext::minimal(|_| anyhow::bail!("no backend"));

        assert!(
            resolve_metadata("[RJ123456] Placeholder Game.zip", &ctx).is_none(),
            "metadata must not resolve without the feature that provides the store"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn folder_staging_copy_failure_preserves_existing_output() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        let link = source.join("linked.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, &link)
            .expect("Windows symlink support is required for this containment regression");

        let destination = temp.path().join("output");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("save.dat"), b"known-good").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();

        let error = copy_dir_recursive(&source, staged.artifact_path())
            .unwrap_err()
            .to_string();
        drop(staged);

        assert!(error.contains("symlink or special"), "{error}");
        assert_eq!(
            std::fs::read(destination.join("save.dat")).unwrap(),
            b"known-good"
        );
    }
}
