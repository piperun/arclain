//! Blocking pipeline executor — runs a `Pipeline` against each input file.

use super::context::PipelineContext;
use super::types::{Pipeline, PipelineInput, PipelineStep};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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
    FileFailed {
        error: String,
    },
    AllComplete {
        succeeded: usize,
        failed: usize,
    },
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
            Ok(output) => {
                succeeded += 1;
                on_progress(PipelineProgress::FileComplete { output });
            }
            Err(e) => {
                failed += 1;
                on_progress(PipelineProgress::FileFailed {
                    error: e.to_string(),
                });
            }
        }
    }

    on_progress(PipelineProgress::AllComplete { succeeded, failed });
    Ok(())
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
