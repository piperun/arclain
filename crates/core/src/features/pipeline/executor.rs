//! Blocking pipeline executor — runs a `Pipeline` against each input file.

use super::context::PipelineContext;
use super::hashing::hash_file_blake3_with_progress;
use super::types::{
    OutputArtifact, OutputCollisionPolicy, Pipeline, PipelineInput, PipelineStep,
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
    //
    // Hashing a multi-GB archive is the first visible delay in the pipeline,
    // so emit a dedicated step + progress events so the UI doesn't look frozen.
    on_progress(PipelineProgress::StepStart {
        step_index: 0,
        step_name: "Hashing input".to_string(),
    });
    let (input_blake3, input_size) =
        hash_file_blake3_with_progress(input, |percent| {
            on_progress(PipelineProgress::StepProgress { percent });
        })
        .with_context(|| format!("hash input {:?}", input))?;
    let pipeline_hash = pipeline.config_hash();

    // Predict the final output path. For Archive mode we use the last Convert
    // step's extension (or zip as the fallback). For Folder mode the artifact
    // is a directory named after the input's stem.
    let final_format = last_convert_format(&pipeline.steps)
        .unwrap_or(crate::features::conversion::ConvertFormat::Zip);
    let predicted_output_path = match pipeline.output_artifact {
        OutputArtifact::Archive => pipeline.output.resolve(input, final_format.extension()),
        OutputArtifact::Folder => pipeline.output.resolve_folder(input),
    };

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

    // Finalize. Two paths:
    //   Archive → pack work_dir via 7z CLI (historical behavior)
    //   Folder  → move work_dir to the output location, no repacking
    match pipeline.output_artifact {
        OutputArtifact::Archive => {
            let format =
                final_format.unwrap_or(crate::features::conversion::ConvertFormat::Zip);
            let output_path = pipeline.output.resolve(input, format.extension());

            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            on_progress(PipelineProgress::StepStart {
                step_index: pipeline.steps.len() + 1,
                step_name: format!("Packing .{}", format.extension()),
            });

            let cli = crate::backends::SevenZipCli::detect(None)
                .context("7z CLI not found — required for pipeline conversion")?;
            let handle = cli
                .spawn_convert_with_progress(&work_dir, &output_path, format, final_compression)
                .context("Failed to spawn 7z compression")?;
            drain_progress(handle, on_progress)?;

            Ok(output_path)
        }
        OutputArtifact::Folder => {
            let output_path = pipeline.output.resolve_folder(input);

            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            on_progress(PipelineProgress::StepStart {
                step_index: pipeline.steps.len() + 1,
                step_name: "Writing folder output".to_string(),
            });

            // Rename the work dir into place when we can; on cross-device moves
            // fall back to copy + delete. Either way, `output_path` is
            // guaranteed not to exist here — the collision gate above either
                // removed it (Overwrite) or skipped the whole run.
            move_dir(&work_dir, &output_path)
                .with_context(|| format!("Moving {:?} to {:?}", work_dir, output_path))?;
            on_progress(PipelineProgress::StepProgress { percent: 100 });

            Ok(output_path)
        }
    }
}

/// Move a directory tree. Falls back to recursive copy + delete if `rename`
/// fails (typically because the destination is on a different filesystem).
fn move_dir(src: &Path, dest: &Path) -> Result<()> {
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => move_dir_via_copy(src, dest),
    }
}

/// Cross-device fallback: copy the tree, then remove the source.
fn move_dir_via_copy(src: &Path, dest: &Path) -> Result<()> {
    copy_dir_recursive(src, dest)?;
    std::fs::remove_dir_all(src).ok();
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("Creating {:?}", dest))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("Reading {:?}", src))? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("Copying {:?} to {:?}", from, to))?;
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

    /// Regression test for C3 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// `move_dir_via_copy` (the cross-device fallback) calls
    /// `std::fs::remove_dir_all(src).ok()` — the `.ok()` silently swallows
    /// any removal error. After the cross-device copy succeeds, if removal
    /// of the source then fails, the function still returns `Ok(())` and
    /// the caller sees no signal that the source directory still exists.
    /// Subsequent pipeline runs see the leftover work-dir and may corrupt
    /// `WorkDirGuard` cleanup or duplicate archives in the user's library.
    ///
    /// This test forces removal to fail by holding an exclusive child
    /// process or, on Unix, by removing write permission on the parent of
    /// `src` so that unlinking entries inside `src` is impossible.
    /// It then asserts that `move_dir_via_copy` returns `Ok` despite `src`
    /// still existing — confirming the silent-failure pattern.
    ///
    /// After the C3 fix (replace `.ok()` with `.with_context(...)?`), this
    /// test should be updated to assert the function returns `Err`.
    #[cfg(unix)]
    #[test]
    fn c3_move_dir_via_copy_silently_swallows_remove_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        // Layout: temp/parent/src/file.txt
        // We chmod `parent` to read+execute only (0o555). That blocks any
        // unlink inside `parent`, so `remove_dir_all(parent/src)` fails on
        // its first attempt to remove a child inside the locked parent.
        let parent = temp.path().join("parent");
        std::fs::create_dir_all(&parent).unwrap();
        let src = parent.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("file.txt"), "hello").unwrap();
        let dest = temp.path().join("dest");

        // Strip write+exec from parent so children can't be unlinked.
        // (read+execute = traversal only)
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&parent, perms).unwrap();

        let result = move_dir_via_copy(&src, &dest);

        // Restore perms so tempdir cleanup succeeds.
        let mut perms = std::fs::metadata(&parent).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&parent, perms).unwrap();

        assert!(
            result.is_ok(),
            "C3 not reproduced: move_dir_via_copy returned Err — \
             either remove unexpectedly succeeded or the .ok() was already replaced",
        );
        assert!(
            src.exists(),
            "C3 not reproduced: src was removed despite locked parent — \
             the test fault setup didn't actually block removal",
        );
        assert!(
            dest.join("file.txt").exists(),
            "Sanity: dest copy must have succeeded for this test to be meaningful",
        );
    }

    /// Windows variant of the C3 regression test. Uses an open `File` handle
    /// without `FILE_SHARE_DELETE` to block deletion. Rust's
    /// `std::fs::File::open` shares delete by default on Windows, so we use
    /// the `OpenOptions` extension to explicitly request exclusive access.
    ///
    /// If this test ends up flaky on a given Windows version (for example
    /// due to `remove_dir_all` retrying internally), it should be replaced
    /// with a dedicated fault-injection helper.
    #[cfg(windows)]
    #[test]
    fn c3_move_dir_via_copy_silently_swallows_remove_failure() {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        // Note: deliberately NOT including FILE_SHARE_DELETE (0x4).

        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file_path = src.join("locked.txt");
        std::fs::write(&file_path, "hello").unwrap();
        let dest = temp.path().join("dest");

        // Open the file without delete-share. Holding this handle blocks
        // any other process (including this one's remove_dir_all) from
        // unlinking the file.
        let _locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&file_path)
            .expect("opening file without delete-share");

        let result = move_dir_via_copy(&src, &dest);

        assert!(
            result.is_ok(),
            "C3 not reproduced: move_dir_via_copy returned Err — \
             either the OS allowed deletion of the open handle or the .ok() \
             was already replaced. Got: {:?}",
            result,
        );
        assert!(
            src.exists(),
            "C3 not reproduced: src was removed despite the held file handle — \
             the OS may have queued deletion (DELETE_ON_CLOSE) after the handle drops",
        );
        assert!(
            dest.join("locked.txt").exists(),
            "Sanity: dest copy must have succeeded for this test to be meaningful",
        );

        // Drop the handle so tempdir cleanup can succeed.
        drop(_locked);
    }
}
