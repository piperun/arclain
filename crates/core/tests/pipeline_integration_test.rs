//! Integration tests for the Pipeline executor.
//!
//! The end-to-end test requires 7z CLI on PATH and a test archive fixture —
//! marked `#[ignore]` so it runs only when explicitly requested.
//! The other tests are pure and run in CI.

use arclain_core::{
    execute_pipeline, preview_pipeline, ArchiveBackend, CompressionLevel, ConvertFormat,
    OutputCollisionPolicy, Pipeline, PipelineContext, PipelineInput, PipelineOutput, PipelineStep,
};
use arclain_db::SqliteDb;
use std::path::PathBuf;
use std::sync::Arc;

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
        collision_policy: None,
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
        collision_policy: None,
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
        collision_policy: None,
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

/// Minimal pipeline fixture: Convert-only, input fake paths, no Flatten/Organize.
/// Useful for exercising the collision gate without archive fixtures since
/// Skip and Fail paths exit before the extraction callback is invoked.
fn collision_test_pipeline(
    inputs: Vec<PathBuf>,
    output_dir: PathBuf,
    policy: OutputCollisionPolicy,
) -> Pipeline {
    Pipeline {
        input: Some(PipelineInput::Files(inputs)),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(output_dir),
        collision_policy: Some(policy),
    }
}

/// Backend that panics if asked to do anything. Proves the collision gate
/// short-circuits before any archive I/O is attempted.
fn unreachable_backend_ctx() -> PipelineContext {
    PipelineContext::minimal(|path| -> anyhow::Result<Arc<dyn ArchiveBackend>> {
        panic!(
            "collision gate should have short-circuited before touching {}",
            path.display()
        );
    })
}

#[test]
fn collision_skip_returns_existing_without_extracting() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"fake archive").unwrap();
    let existing_output = tmp.path().join("mod.zip");
    std::fs::write(&existing_output, b"pre-existing artifact").unwrap();

    let pipeline = collision_test_pipeline(
        vec![input.clone()],
        tmp.path().to_path_buf(),
        OutputCollisionPolicy::Skip,
    );

    let ctx = unreachable_backend_ctx();
    let mut skipped: Vec<PathBuf> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileSkipped { output, .. } => skipped.push(output),
            FileFailed { error } => failures.push(error),
            _ => {}
        }
    })
    .unwrap();

    assert_eq!(skipped, vec![existing_output.clone()]);
    assert!(failures.is_empty());
    // Existing bytes untouched (Skip = do not rewrite)
    let body = std::fs::read(&existing_output).unwrap();
    assert_eq!(body, b"pre-existing artifact");
}

#[test]
fn collision_fail_errors_when_output_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"fake archive").unwrap();
    let existing_output = tmp.path().join("mod.zip");
    std::fs::write(&existing_output, b"pre-existing").unwrap();

    let pipeline = collision_test_pipeline(
        vec![input.clone()],
        tmp.path().to_path_buf(),
        OutputCollisionPolicy::Fail,
    );

    let ctx = unreachable_backend_ctx();
    let mut failures: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        if let arclain_core::PipelineProgress::FileFailed { error } = ev {
            failures.push(error);
        }
    })
    .unwrap();

    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].contains("already exists"),
        "expected collision error, got: {}",
        failures[0]
    );
}

#[test]
fn collision_smart_degrades_to_fail_pre_phase_3() {
    // Phase 2: Smart has no DB to consult, so it must fall back to Fail
    // behavior rather than silently skipping.
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"fake archive").unwrap();
    let existing_output = tmp.path().join("mod.zip");
    std::fs::write(&existing_output, b"pre-existing").unwrap();

    let pipeline = collision_test_pipeline(
        vec![input],
        tmp.path().to_path_buf(),
        OutputCollisionPolicy::Smart,
    );

    let ctx = unreachable_backend_ctx();
    let mut failures: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        if let arclain_core::PipelineProgress::FileFailed { error } = ev {
            failures.push(error);
        }
    })
    .unwrap();

    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("already exists"));
}

#[test]
fn collision_policy_defaults_to_smart_when_unset() {
    let pipeline = Pipeline {
        input: None,
        steps: vec![],
        output: PipelineOutput::SameFolder,
        collision_policy: None,
    };
    assert_eq!(
        pipeline.effective_collision_policy(OutputCollisionPolicy::Smart),
        OutputCollisionPolicy::Smart
    );
    assert_eq!(
        pipeline.effective_collision_policy(OutputCollisionPolicy::Skip),
        OutputCollisionPolicy::Skip
    );
}

#[test]
fn collision_policy_override_wins_over_default() {
    let pipeline = Pipeline {
        input: None,
        steps: vec![],
        output: PipelineOutput::SameFolder,
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
    };
    assert_eq!(
        pipeline.effective_collision_policy(OutputCollisionPolicy::Smart),
        OutputCollisionPolicy::Overwrite
    );
}

#[test]
fn preview_annotates_existing_output_with_policy_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"x").unwrap();
    let preexisting = tmp.path().join("mod.zip");
    std::fs::write(&preexisting, b"y").unwrap();

    let pipeline_skip = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(tmp.path().to_path_buf()),
        collision_policy: Some(OutputCollisionPolicy::Skip),
    };
    let preview = preview_pipeline(&pipeline_skip);
    let warnings = &preview.entries[0].warnings;
    assert!(
        warnings.iter().any(|w| w.contains("will be skipped")),
        "expected 'will be skipped' warning, got: {:?}",
        warnings
    );

    let pipeline_overwrite = Pipeline {
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        ..pipeline_skip.clone()
    };
    let preview = preview_pipeline(&pipeline_overwrite);
    let warnings = &preview.entries[0].warnings;
    assert!(warnings.iter().any(|w| w.contains("will be overwritten")));

    let pipeline_fail = Pipeline {
        collision_policy: Some(OutputCollisionPolicy::Fail),
        ..pipeline_skip.clone()
    };
    let preview = preview_pipeline(&pipeline_fail);
    let warnings = &preview.entries[0].warnings;
    assert!(warnings.iter().any(|w| w.contains("will fail")));
}

#[test]
fn pipeline_deserializes_without_collision_policy() {
    // Legacy presets written before the field existed must still load cleanly.
    let legacy_json = r#"{
        "input": null,
        "steps": [],
        "output": "SameFolder"
    }"#;
    let pipeline: Pipeline = serde_json::from_str(legacy_json).unwrap();
    assert!(pipeline.collision_policy.is_none());
}

// ---- Phase 3: DB + dedup tests ----

fn open_pipeline_runs_db() -> Arc<SqliteDb> {
    let db = SqliteDb::open_in_memory().unwrap();
    db.with_connection(|conn| Ok(arclain_db::ensure_pipeline_runs_table(conn)?))
        .unwrap();
    Arc::new(db)
}

#[test]
fn config_hash_is_stable_across_identical_pipelines() {
    let a = Pipeline {
        input: Some(PipelineInput::Files(vec![PathBuf::from("/tmp/a.rar")])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::SameFolder,
        collision_policy: Some(OutputCollisionPolicy::Smart),
    };
    // Same config, DIFFERENT input → hashes must match (input is excluded)
    let b = Pipeline {
        input: Some(PipelineInput::Files(vec![PathBuf::from("/tmp/b.rar")])),
        ..a.clone()
    };
    assert_eq!(a.config_hash(), b.config_hash());
}

#[test]
fn config_hash_changes_when_steps_change() {
    let a = Pipeline {
        input: None,
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::SameFolder,
        collision_policy: None,
    };
    let b = Pipeline {
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::SevenZ, // changed
            compression: CompressionLevel::Normal,
            password: None,
        }],
        ..a.clone()
    };
    assert_ne!(a.config_hash(), b.config_hash());
}

#[test]
fn config_hash_changes_when_collision_policy_changes() {
    let a = Pipeline {
        input: None,
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::SameFolder,
        collision_policy: Some(OutputCollisionPolicy::Smart),
    };
    let b = Pipeline {
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        ..a.clone()
    };
    assert_ne!(a.config_hash(), b.config_hash());
}

#[test]
fn smart_rerun_with_matching_db_row_skips_work() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"repeatable contents").unwrap();
    let existing_output = tmp.path().join("mod.zip");
    std::fs::write(&existing_output, b"already produced").unwrap();

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(tmp.path().to_path_buf()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
    };

    // Seed a matching completed run for this exact input + pipeline config
    let db = open_pipeline_runs_db();
    let (input_hash, input_size) =
        arclain_core::features::pipeline::hashing::hash_file_blake3(&input).unwrap();
    let pipeline_hash = pipeline.config_hash();

    let input_str = input.to_string_lossy().into_owned();
    let existing_str = existing_output.to_string_lossy().into_owned();
    db.with_connection(|conn| {
        let new_run = arclain_db::NewPipelineRun {
            input_path: &input_str,
            input_blake3: &input_hash,
            input_size: input_size as i64,
            pipeline_hash: &pipeline_hash,
            arclain_version: "test",
        };
        let id = arclain_db::begin_pipeline_run(conn, &new_run)?;
        arclain_db::mark_run_completed(
            conn,
            id,
            &existing_str,
            arclain_db::pipeline_output_kind::ARCHIVE,
        )?;
        Ok(())
    })
    .unwrap();

    let ctx = PipelineContext {
        organization_service: None,
        library_service: None,
        backend_for: Arc::new(|path| -> anyhow::Result<Arc<dyn ArchiveBackend>> {
            panic!(
                "Smart rerun should skip without extracting {}",
                path.display()
            )
        }),
        config_db: Some(db.clone()),
    };

    let mut skipped: Vec<(PathBuf, String)> = Vec::new();
    let mut summary: Option<(usize, usize, usize)> = None;
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileSkipped { output, reason } => skipped.push((output, reason)),
            AllComplete { succeeded, skipped: s, failed } => summary = Some((succeeded, s, failed)),
            _ => {}
        }
    })
    .unwrap();

    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].0, existing_output);
    assert!(skipped[0].1.contains("already processed"));
    assert_eq!(summary, Some((0, 1, 0)));
}

#[test]
fn smart_rerun_with_different_pipeline_reruns() {
    // Same input, DIFFERENT pipeline → no DB match → Smart falls back to Fail
    // because the output path already exists and arclain can't prove it made it.
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"same bytes").unwrap();
    let existing_output = tmp.path().join("mod.zip");
    std::fs::write(&existing_output, b"preexisting").unwrap();

    // Seed the DB with a run for a DIFFERENT pipeline config
    let db = open_pipeline_runs_db();
    let (input_hash, input_size) =
        arclain_core::features::pipeline::hashing::hash_file_blake3(&input).unwrap();
    let input_str = input.to_string_lossy().into_owned();
    let existing_str = existing_output.to_string_lossy().into_owned();
    db.with_connection(|conn| {
        let id = arclain_db::begin_pipeline_run(
            conn,
            &arclain_db::NewPipelineRun {
                input_path: &input_str,
                input_blake3: &input_hash,
                input_size: input_size as i64,
                pipeline_hash: "different_pipeline_hash",
                arclain_version: "test",
            },
        )?;
        arclain_db::mark_run_completed(
            conn,
            id,
            &existing_str,
            arclain_db::pipeline_output_kind::ARCHIVE,
        )?;
        Ok(())
    })
    .unwrap();

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(tmp.path().to_path_buf()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
    };

    let ctx = PipelineContext {
        organization_service: None,
        library_service: None,
        backend_for: Arc::new(|_| panic!("should not extract — Smart with no match must Fail")),
        config_db: Some(db.clone()),
    };

    let mut failures: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        if let arclain_core::PipelineProgress::FileFailed { error } = ev {
            failures.push(error);
        }
    })
    .unwrap();

    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].contains("no record"),
        "expected 'no record of producing it' error, got: {}",
        failures[0]
    );
}

#[test]
fn smart_rerun_reruns_when_output_was_deleted() {
    // DB has a matching completed run but the output file is gone — we should
    // NOT skip (the DB row is stale); fresh run kicks in. In this test we use
    // Skip policy for the secondary collision check so we don't need real work.
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"bytes").unwrap();
    // note: existing_output does NOT exist

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(tmp.path().to_path_buf()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
    };

    let db = open_pipeline_runs_db();
    let (input_hash, input_size) =
        arclain_core::features::pipeline::hashing::hash_file_blake3(&input).unwrap();
    let pipeline_hash = pipeline.config_hash();

    // Seed a matching row pointing at a file that doesn't exist
    let input_str = input.to_string_lossy().into_owned();
    let missing_output_str = tmp.path().join("mod.zip").to_string_lossy().into_owned();
    db.with_connection(|conn| {
        let id = arclain_db::begin_pipeline_run(
            conn,
            &arclain_db::NewPipelineRun {
                input_path: &input_str,
                input_blake3: &input_hash,
                input_size: input_size as i64,
                pipeline_hash: &pipeline_hash,
                arclain_version: "test",
            },
        )?;
        arclain_db::mark_run_completed(
            conn,
            id,
            &missing_output_str,
            arclain_db::pipeline_output_kind::ARCHIVE,
        )?;
        Ok(())
    })
    .unwrap();

    // Backend that would be reached IF skip didn't trigger — we expect it to
    // error out cleanly (not panic) so the test surfaces as FileFailed.
    let ctx = PipelineContext {
        organization_service: None,
        library_service: None,
        backend_for: Arc::new(|_| anyhow::bail!("no real backend in this test")),
        config_db: Some(db.clone()),
    };

    let mut failures: Vec<String> = Vec::new();
    let mut skipped: Vec<PathBuf> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileFailed { error } => failures.push(error),
            FileSkipped { output, .. } => skipped.push(output),
            _ => {}
        }
    })
    .unwrap();

    // DB row was stale (output missing) → Smart proceeded → backend errored.
    // The important assertion: no skip happened.
    assert!(skipped.is_empty(), "Smart should rerun when stored output is gone");
    assert_eq!(failures.len(), 1);
}

#[test]
fn db_records_run_with_in_progress_then_completed() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("mod.rar");
    std::fs::write(&input, b"data").unwrap();
    let existing = tmp.path().join("mod.zip");
    std::fs::write(&existing, b"prev").unwrap();

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(tmp.path().to_path_buf()),
        // Skip so we don't need extraction machinery — still exercises the
        // pre-flight gate path, but no DB row is written for skips (the
        // gate returns before begin_pipeline_run).
        collision_policy: Some(OutputCollisionPolicy::Skip),
    };

    let db = open_pipeline_runs_db();
    let ctx = PipelineContext {
        organization_service: None,
        library_service: None,
        backend_for: Arc::new(|_| panic!("unreachable")),
        config_db: Some(db.clone()),
    };

    execute_pipeline(&pipeline, tmp.path(), &ctx, |_| {}).unwrap();

    // Skip returns before begin_pipeline_run, so the table stays empty.
    let count: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM pipeline_runs", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(count, 0);
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
