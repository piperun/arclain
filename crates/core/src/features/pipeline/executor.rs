//! Blocking pipeline executor — runs a `Pipeline` against each input file.

use super::types::{Pipeline, PipelineInput, PipelineStep};
use crate::archive::ArchiveBackend;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
///
/// `backend_for` is a callback that returns the appropriate backend for a given
/// file path (UI layer provides this to avoid pulling backend selector into core).
pub fn execute_pipeline<F>(
    pipeline: &Pipeline,
    temp_root: &Path,
    backend_for: F,
    mut on_progress: impl FnMut(PipelineProgress),
) -> Result<()>
where
    F: Fn(&Path) -> Result<Arc<dyn ArchiveBackend>>,
{
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

        match run_one(input, pipeline, temp_root, &backend_for, &mut on_progress) {
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

fn run_one<F>(
    input: &Path,
    pipeline: &Pipeline,
    temp_root: &Path,
    backend_for: &F,
    on_progress: &mut impl FnMut(PipelineProgress),
) -> Result<PathBuf>
where
    F: Fn(&Path) -> Result<Arc<dyn ArchiveBackend>>,
{
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
    let source_backend = backend_for(input)?;
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
            } => {
                crate::features::conversion::flatten::flatten_nested_archives(
                    &work_dir,
                    *strip_common_prefix,
                    |archive_path, dest_dir| {
                        let be = backend_for(archive_path)?;
                        be.extract_all(archive_path, dest_dir, None)
                    },
                )?;
            }
            PipelineStep::Organize { rule_id: _ } => {
                // Intentional no-op in 1.2: organization requires rule engine
                // + metadata resolution. Slated for 1.3.
                anyhow::bail!("Organize step not yet implemented in executor");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_rejects_no_input() {
        let p = Pipeline::default();
        let tmp = tempfile::tempdir().unwrap();
        let result = execute_pipeline(&p, tmp.path(), |_| anyhow::bail!("no backend"), |_| {});
        assert!(result.is_err());
    }
}
