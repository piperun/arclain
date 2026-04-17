//! Core data types for archive processing pipelines.

use crate::features::conversion::{CompressionLevel, ConvertFormat};
use std::path::{Path, PathBuf};

/// A single operation that can appear in a pipeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineStep {
    /// Unwrap inner archives as sibling folders.
    Flatten { strip_common_prefix: bool },
    /// Apply an organization rule by its database id.
    Organize { rule_id: i64 },
    /// Convert the final layout to a target format.
    Convert {
        format: ConvertFormat,
        compression: CompressionLevel,
        password: Option<String>,
    },
}

impl PipelineStep {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Flatten { .. } => "Flatten nested archives",
            Self::Organize { .. } => "Apply organization rule",
            Self::Convert { .. } => "Convert format",
        }
    }
}

/// What the pipeline operates on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineInput {
    /// One or more explicit archive files.
    Files(Vec<PathBuf>),
    /// All archives inside a folder (non-recursive).
    Folder(PathBuf),
}

impl PipelineInput {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Files(v) => v.is_empty(),
            Self::Folder(p) => !p.exists(),
        }
    }
}

/// Where pipeline outputs land.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineOutput {
    /// Output next to each input (e.g. input.rar → input.zip).
    SameFolder,
    /// Output to a specific folder.
    NewFolder(PathBuf),
}

impl Default for PipelineOutput {
    fn default() -> Self {
        Self::SameFolder
    }
}

impl PipelineOutput {
    /// Resolve the output path for a given input file with the target extension.
    pub fn resolve(&self, input: &Path, ext: &str) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default();
        match self {
            Self::SameFolder => {
                let mut p = input.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let mut name = stem.to_os_string();
                name.push(format!(".{}", ext));
                p.push(name);
                p
            }
            Self::NewFolder(folder) => {
                let mut p = folder.clone();
                let mut name = stem.to_os_string();
                name.push(format!(".{}", ext));
                p.push(name);
                p
            }
        }
    }
}

/// Complete pipeline specification.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pipeline {
    pub input: Option<PipelineInput>,
    pub steps: Vec<PipelineStep>,
    pub output: PipelineOutput,
}

/// Preset for opening the Process page pre-configured.
/// Used by the toolbar shortcuts (Convert..., Organize, etc.).
#[derive(Debug, Clone)]
pub enum ProcessPreset {
    /// Opens with Convert step added.
    ConvertSingleFile(PathBuf),
    /// Opens with folder input populated, no steps.
    BatchFolder(PathBuf),
    /// Opens with Organize step for the current archive.
    OrganizeSingleFile(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_display_names() {
        assert_eq!(
            PipelineStep::Flatten {
                strip_common_prefix: false
            }
            .display_name(),
            "Flatten nested archives"
        );
    }

    #[test]
    fn default_pipeline_is_empty() {
        let p = Pipeline::default();
        assert!(p.input.is_none());
        assert!(p.steps.is_empty());
        assert_eq!(p.output, PipelineOutput::SameFolder);
    }

    #[test]
    fn input_is_empty_variants() {
        assert!(PipelineInput::Files(vec![]).is_empty());
        assert!(!PipelineInput::Files(vec![PathBuf::from("a.rar")]).is_empty());
    }

    #[test]
    fn output_resolve_same_folder() {
        let input = PathBuf::from("/src/mod.rar");
        let output = PipelineOutput::SameFolder;
        assert_eq!(output.resolve(&input, "zip"), PathBuf::from("/src/mod.zip"));
    }

    #[test]
    fn output_resolve_new_folder() {
        let input = PathBuf::from("/src/mod.rar");
        let output = PipelineOutput::NewFolder(PathBuf::from("/dst"));
        assert_eq!(output.resolve(&input, "7z"), PathBuf::from("/dst/mod.7z"));
    }
}
