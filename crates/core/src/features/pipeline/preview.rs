//! Pure pipeline preview — stub for Task 1, implemented in Task 2.

use super::types::Pipeline;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewEntry {
    pub input: PathBuf,
    pub operations: Vec<String>,
    pub expected_output: Option<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PipelinePreview {
    pub entries: Vec<PreviewEntry>,
    pub global_warnings: Vec<String>,
}

impl PipelinePreview {
    pub fn total_files(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn preview_pipeline(_pipeline: &Pipeline) -> PipelinePreview {
    PipelinePreview::default()
}
