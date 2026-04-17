//! Blocking pipeline executor — stub for Task 1, implemented in Task 3.

use super::types::Pipeline;
use std::path::{Path, PathBuf};

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

pub fn execute_pipeline<F>(
    _pipeline: &Pipeline,
    _temp_root: &Path,
    _backend_for: F,
    _on_progress: impl FnMut(PipelineProgress),
) -> anyhow::Result<()>
where
    F: Fn(&Path) -> anyhow::Result<std::sync::Arc<dyn crate::archive::ArchiveBackend>>,
{
    anyhow::bail!("executor not yet implemented (Task 3)")
}
