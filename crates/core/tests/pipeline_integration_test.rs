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
                max_depth: 1,
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

#[test]
fn apply_plan_reorganizes_files() {
    use arclain_core::features::organization::engine::OrganizationPlan;
    use arclain_core::features::pipeline::apply_plan::apply_plan_to_workdir;
    use std::fs;

    let tmp = tempfile::tempdir().unwrap();

    // Pre-populate work dir as if extraction just finished
    fs::write(tmp.path().join("game.exe"), b"").unwrap();
    fs::create_dir(tmp.path().join("data")).unwrap();
    fs::write(tmp.path().join("data/sprites.dat"), b"").unwrap();

    let plan = OrganizationPlan {
        rule_name: "test".into(),
        root_folder: "MyGame".into(),
        root_folder_template: "MyGame".into(),
        moves: vec![
            ("game.exe".into(), "MyGame/game.exe".into()),
            ("data/sprites.dat".into(), "MyGame/data/sprites.dat".into()),
        ],
        generated_files: vec![(
            "MyGame/metadata.json".into(),
            r#"{"title":"Test"}"#.into(),
        )],
        downloads: vec![],
        use_standard_layout: true,
        resolved_variables: Default::default(),
    };

    apply_plan_to_workdir(&plan, tmp.path()).unwrap();

    assert!(tmp.path().join("MyGame/game.exe").exists());
    assert!(tmp.path().join("MyGame/data/sprites.dat").exists());
    assert!(tmp.path().join("MyGame/metadata.json").exists());
    assert!(!tmp.path().join("game.exe").exists());
    // Old empty top-level data/ folder should be gone after flatten
    assert!(!tmp.path().join("data").exists());
}
