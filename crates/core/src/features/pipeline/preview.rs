//! Pure pipeline preview — computes what the pipeline WILL do without running it.

use super::types::{OutputArtifact, OutputCollisionPolicy, Pipeline, PipelineInput, PipelineStep};
use crate::features::conversion::ConvertFormat;
use std::path::PathBuf;

/// Preview of one input file going through the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewEntry {
    pub input: PathBuf,
    pub operations: Vec<String>,
    pub expected_output: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// Complete preview result for a pipeline.
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

/// Compute the preview for a pipeline.
pub fn preview_pipeline(pipeline: &Pipeline) -> PipelinePreview {
    let mut preview = PipelinePreview::default();

    let inputs: Vec<PathBuf> = match &pipeline.input {
        None => return preview,
        Some(PipelineInput::Files(v)) => v.clone(),
        Some(PipelineInput::Folder(p)) => {
            match crate::features::conversion::flatten::find_archive_files(p) {
                Ok(v) if v.is_empty() => {
                    preview
                        .global_warnings
                        .push(format!("No archives found in {:?}", p));
                    return preview;
                }
                Ok(v) => v,
                Err(e) => {
                    preview.global_warnings.push(format!("Scan failed: {}", e));
                    return preview;
                }
            }
        }
    };

    if pipeline.steps.is_empty() {
        preview
            .global_warnings
            .push("No operations added".to_string());
    }

    for input in inputs {
        let mut entry = PreviewEntry {
            input: input.clone(),
            operations: Vec::new(),
            expected_output: None,
            warnings: Vec::new(),
        };

        let mut final_format: Option<ConvertFormat> = None;

        for step in &pipeline.steps {
            match step {
                PipelineStep::Flatten {
                    strip_common_prefix,
                    max_depth,
                } => {
                    let base = if *strip_common_prefix {
                        "Flatten nested archives (strip common prefix)"
                    } else {
                        "Flatten nested archives"
                    };
                    entry.operations.push(match max_depth {
                        0 => format!("{} (recursive)", base),
                        1 => base.to_string(),
                        n => format!("{} (up to {} passes)", base, n),
                    });
                }
                PipelineStep::Organize { rule_id } => {
                    entry
                        .operations
                        .push(format!("Apply organization rule #{}", rule_id));
                }
                PipelineStep::Convert { format, .. } => {
                    entry
                        .operations
                        .push(format!("Convert to .{}", format.extension()));
                    final_format = Some(format.clone());
                }
            }
        }

        entry.expected_output = match pipeline.output_artifact {
            OutputArtifact::Archive => final_format
                .map(|fmt| pipeline.output.resolve(&input, fmt.extension())),
            OutputArtifact::Folder => Some(pipeline.output.resolve_folder(&input)),
        };

        if let Some(ref out) = entry.expected_output {
            if out.exists() && out != &input {
                let policy = pipeline.effective_collision_policy(OutputCollisionPolicy::Smart);
                let outcome = match policy {
                    OutputCollisionPolicy::Skip => "will be skipped".to_string(),
                    OutputCollisionPolicy::Overwrite => "will be overwritten".to_string(),
                    OutputCollisionPolicy::Fail | OutputCollisionPolicy::Smart => {
                        "will fail this file (change policy to Overwrite or Skip)".to_string()
                    }
                };
                entry.warnings.push(format!(
                    "Output already exists — {}: {}",
                    outcome,
                    out.display()
                ));
            }
        }

        preview.entries.push(entry);
    }

    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::conversion::{CompressionLevel, ConvertFormat};
    use crate::features::pipeline::types::{Pipeline, PipelineInput, PipelineOutput};

    #[test]
    fn empty_pipeline_no_input() {
        let p = Pipeline::default();
        let preview = preview_pipeline(&p);
        assert!(preview.is_empty());
    }

    #[test]
    fn pipeline_with_input_no_steps_warns() {
        let p = Pipeline {
            input: Some(PipelineInput::Files(vec![PathBuf::from("/tmp/a.rar")])),
            steps: vec![],
            output: PipelineOutput::SameFolder,
            collision_policy: None,
            output_artifact: Default::default(),
        };
        let preview = preview_pipeline(&p);
        assert!(preview
            .global_warnings
            .iter()
            .any(|w| w.contains("No operations")));
    }

    #[test]
    fn convert_only_produces_output_path() {
        let p = Pipeline {
            input: Some(PipelineInput::Files(vec![PathBuf::from("/tmp/mod.rar")])),
            steps: vec![PipelineStep::Convert {
                format: ConvertFormat::Zip,
                compression: CompressionLevel::Normal,
                password: None,
            }],
            output: PipelineOutput::SameFolder,
            collision_policy: None,
            output_artifact: Default::default(),
        };
        let preview = preview_pipeline(&p);
        assert_eq!(preview.entries.len(), 1);
        let entry = &preview.entries[0];
        assert_eq!(entry.operations, vec!["Convert to .zip"]);
        assert_eq!(entry.expected_output, Some(PathBuf::from("/tmp/mod.zip")));
    }

    #[test]
    fn flatten_plus_convert_shows_both_ops() {
        let p = Pipeline {
            input: Some(PipelineInput::Files(vec![PathBuf::from("/tmp/pack.rar")])),
            steps: vec![
                PipelineStep::Flatten {
                    strip_common_prefix: true,
                    max_depth: 1,
                },
                PipelineStep::Convert {
                    format: ConvertFormat::Zip,
                    compression: CompressionLevel::Normal,
                    password: None,
                },
            ],
            output: PipelineOutput::SameFolder,
            collision_policy: None,
            output_artifact: Default::default(),
        };
        let preview = preview_pipeline(&p);
        let entry = &preview.entries[0];
        assert_eq!(entry.operations.len(), 2);
        assert!(entry.operations[0].contains("Flatten"));
        assert!(entry.operations[1].contains("Convert"));
    }

    #[test]
    fn new_folder_output_changes_path() {
        let p = Pipeline {
            input: Some(PipelineInput::Files(vec![PathBuf::from("/src/mod.rar")])),
            steps: vec![PipelineStep::Convert {
                format: ConvertFormat::SevenZ,
                compression: CompressionLevel::Normal,
                password: None,
            }],
            output: PipelineOutput::NewFolder(PathBuf::from("/dst")),
            collision_policy: None,
            output_artifact: Default::default(),
        };
        let preview = preview_pipeline(&p);
        assert_eq!(
            preview.entries[0].expected_output,
            Some(PathBuf::from("/dst/mod.7z"))
        );
    }
}
