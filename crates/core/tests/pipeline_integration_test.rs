//! Integration tests for the Pipeline executor.
//!
//! The end-to-end test requires 7z CLI on PATH and a test archive fixture —
//! marked `#[ignore]` so it runs only when explicitly requested.
//! The other tests are pure and run in CI.

use arclain_core::{
    preview_pipeline, CompressionLevel, ConvertFormat, Pipeline, PipelineInput, PipelineOutput,
    PipelineStep,
};
use std::path::PathBuf;

#[test]
fn preview_for_silver_lining_style_input() {
    let inputs = vec![
        PathBuf::from("/tmp/AG - Silver - Main.rar"),
        PathBuf::from("/tmp/AG - Silver - Patch A.rar"),
        PathBuf::from("/tmp/AG - Silver - Patch B.rar"),
    ];

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(inputs)),
        steps: vec![
            PipelineStep::Flatten {
                strip_common_prefix: true,
            },
            PipelineStep::Convert {
                format: ConvertFormat::Zip,
                compression: CompressionLevel::Normal,
                password: None,
            },
        ],
        output: PipelineOutput::SameFolder,
    };

    let preview = preview_pipeline(&pipeline);
    assert_eq!(preview.total_files(), 3);
    for entry in &preview.entries {
        assert_eq!(entry.operations.len(), 2);
        assert!(entry.operations[0].contains("Flatten"));
        assert!(entry.operations[1].contains("Convert"));
        assert!(entry.expected_output.is_some());
        let out = entry.expected_output.as_ref().unwrap();
        assert_eq!(out.extension().and_then(|s| s.to_str()), Some("zip"));
    }
}

#[test]
fn preview_flags_missing_input() {
    let pipeline = Pipeline {
        input: None,
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::SameFolder,
    };
    let preview = preview_pipeline(&pipeline);
    assert!(preview.is_empty());
}

#[test]
fn preview_flags_empty_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let pipeline = Pipeline {
        input: Some(PipelineInput::Folder(tmp.path().to_path_buf())),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::SameFolder,
    };
    let preview = preview_pipeline(&pipeline);
    assert!(preview
        .global_warnings
        .iter()
        .any(|w| w.contains("No archives found")));
}

#[test]
#[ignore = "requires 7z CLI + test fixture — run locally with: cargo test -- --ignored"]
fn executor_end_to_end_convert() {
    // Build a small zip via the zip crate, run pipeline to convert to 7z,
    // verify output exists. Skipped until we have a test fixture.
}
