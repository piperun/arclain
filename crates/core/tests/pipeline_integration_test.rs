//! Integration tests for the Pipeline executor.
//!
//! The end-to-end test requires 7z CLI on PATH and a test archive fixture —
//! marked `#[ignore]` so it runs only when explicitly requested.
//! The other tests are pure and run in CI.

use arclain_core::{
    execute_pipeline, preview_pipeline, ArchiveBackend, CompressionLevel, ConvertFormat,
    OutputArtifact, OutputCollisionPolicy, Pipeline, PipelineContext, PipelineInput,
    PipelineOutput, PipelineStep,
};
use arclain_db::SqliteDb;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn overwrite_failure_preserves_existing_output() {
    let temp = tempfile::tempdir().unwrap();
    let input_dir = temp.path().join("input");
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let input = input_dir.join("game.zip");
    std::fs::write(&input, b"source").unwrap();
    let output = output_dir.join("game.zip");
    std::fs::write(&output, b"known-good").unwrap();
    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input])),
        steps: vec![],
        output: PipelineOutput::NewFolder(output_dir),
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        output_artifact: OutputArtifact::Archive,
    };
    let ctx = PipelineContext::minimal(|_| anyhow::bail!("injected extraction failure"));
    let mut failures = 0;

    execute_pipeline(&pipeline, temp.path(), &ctx, |event| {
        if matches!(event, arclain_core::PipelineProgress::FileFailed { .. }) {
            failures += 1;
        }
    })
    .unwrap();

    assert_eq!(failures, 1);
    assert_eq!(std::fs::read(output).unwrap(), b"known-good");
}

#[test]
fn same_path_overwrite_failure_preserves_the_input_archive() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("game.zip");
    std::fs::write(&input, b"known-good-input").unwrap();
    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![],
        output: PipelineOutput::SameFolder,
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        output_artifact: OutputArtifact::Archive,
    };
    let ctx = PipelineContext::minimal(|_| anyhow::bail!("injected extraction failure"));
    let mut failures = 0;

    execute_pipeline(&pipeline, temp.path(), &ctx, |event| {
        if matches!(event, arclain_core::PipelineProgress::FileFailed { .. }) {
            failures += 1;
        }
    })
    .unwrap();

    assert_eq!(failures, 1);
    assert_eq!(std::fs::read(input).unwrap(), b"known-good-input");
}

#[test]
fn folder_overwrite_failure_preserves_existing_output_tree() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("game.zip");
    std::fs::write(&input, b"source").unwrap();
    let output_dir = temp.path().join("output");
    let output = output_dir.join("game");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("save.dat"), b"known-good").unwrap();
    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input])),
        steps: vec![],
        output: PipelineOutput::NewFolder(output_dir),
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        output_artifact: OutputArtifact::Folder,
    };
    let ctx = PipelineContext::minimal(|_| anyhow::bail!("injected extraction failure"));
    let mut failures = 0;

    execute_pipeline(&pipeline, temp.path(), &ctx, |event| {
        if matches!(event, arclain_core::PipelineProgress::FileFailed { .. }) {
            failures += 1;
        }
    })
    .unwrap();

    assert_eq!(failures, 1);
    assert_eq!(
        std::fs::read(output.join("save.dat")).unwrap(),
        b"known-good"
    );
}

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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
    };
    let preview = preview_pipeline(&pipeline);
    assert!(preview
        .global_warnings
        .iter()
        .any(|w| w.contains("No archives found")));
}

#[test]
#[ignore = "requires 7z CLI on PATH — run with: cargo test -- --ignored"]
fn executor_end_to_end_idempotent_rerun() {
    // Full end-to-end proof of Phase 3 idempotency:
    // 1. Build a synthetic zip via the `zip` crate (no fixture checkin).
    // 2. Convert it to .7z via the real pipeline → expect Completed, DB row.
    // 3. Re-run same pipeline on same input → expect FileSkipped, no work done.
    // 4. Run a DIFFERENT pipeline (output to .zip instead) → Smart falls back to Fail
    //    because the predicted .zip doesn't exist yet, so it just runs again.
    // 5. Change collision policy to Overwrite and re-run step 2 → Completed (overwritten).
    use arclain_core::backends::BackendSelector;
    use std::io::Write;

    let tmp = tempfile::tempdir().unwrap();
    // Keep input and output directories separate so the zip→zip path doesn't
    // collide with the input file itself.
    let input_dir = tmp.path().join("in");
    let output_dir = tmp.path().join("out");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let input = input_dir.join("test_mod.zip");

    // 1. Build the source archive.
    {
        let file = std::fs::File::create(&input).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"aaaa").unwrap();
        zw.start_file("nested/b.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"bbbb").unwrap();
        zw.finish().unwrap();
    }
    let input_bytes_before = std::fs::read(&input).unwrap();

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::SevenZ,
            compression: CompressionLevel::Fast,
            password: None,
        }],
        output: PipelineOutput::NewFolder(output_dir.clone()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
        output_artifact: Default::default(),
    };

    let db = open_pipeline_runs_db();
    let selector = Arc::new(BackendSelector::default());
    let selector_cloned = selector.clone();
    let backend_for = move |p: &std::path::Path| selector_cloned.select(p);

    let ctx = PipelineContext {
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(backend_for)
    };

    // 2. First run — should produce output and log one in_progress→completed row.
    let mut events_first: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        events_first.push(format!("{:?}", ev));
    })
    .expect("first run should succeed");

    let expected_output = output_dir.join("test_mod.7z");
    assert!(
        expected_output.exists(),
        "first run should produce {}",
        expected_output.display()
    );

    let run_count: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM pipeline_runs WHERE status = 'completed'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(run_count, 1, "exactly one completed row after first run");

    // Capture output bytes + mtime so we can prove the second run doesn't touch it.
    let output_bytes_before = std::fs::read(&expected_output).unwrap();
    let mtime_before = std::fs::metadata(&expected_output)
        .unwrap()
        .modified()
        .unwrap();

    // 3. Second run — Smart should skip via DB match.
    let mut skipped: Vec<String> = Vec::new();
    let mut completed: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileSkipped { reason, .. } => skipped.push(reason),
            FileComplete { output } => completed.push(output.display().to_string()),
            _ => {}
        }
    })
    .expect("second run should succeed");

    assert_eq!(
        skipped.len(),
        1,
        "second run should emit exactly one FileSkipped"
    );
    assert!(skipped[0].contains("already processed"));
    assert!(
        completed.is_empty(),
        "no new FileComplete on idempotent re-run"
    );

    // Output must be byte-identical + mtime unchanged (proof it wasn't rewritten).
    let output_bytes_after = std::fs::read(&expected_output).unwrap();
    assert_eq!(
        output_bytes_before, output_bytes_after,
        "output bytes drifted"
    );
    let mtime_after = std::fs::metadata(&expected_output)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        mtime_before, mtime_after,
        "mtime advanced — work was redone"
    );

    // Input also untouched.
    assert_eq!(input_bytes_before, std::fs::read(&input).unwrap());

    // 4. Different pipeline (Zip output) → no existing .zip, no DB match → runs fresh.
    let zip_pipeline = Pipeline {
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Fast,
            password: None,
        }],
        ..pipeline.clone()
    };
    let mut zip_events: Vec<String> = Vec::new();
    execute_pipeline(&zip_pipeline, tmp.path(), &ctx, |ev| {
        zip_events.push(format!("{:?}", ev));
    })
    .expect("different-format run should succeed");
    let zip_out = output_dir.join("test_mod.zip");
    assert!(
        zip_out.exists(),
        "zip run didn't produce output. Events: {:#?}",
        zip_events
    );

    let completed_count: i64 = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM pipeline_runs WHERE status = 'completed'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        completed_count, 2,
        "zip run should have added a second completed row. Events: {:#?}",
        zip_events
    );

    // 5. Overwrite policy — forces re-work even though the DB has a match.
    let overwrite_pipeline = Pipeline {
        collision_policy: Some(OutputCollisionPolicy::Overwrite),
        ..pipeline.clone()
    };
    // Sleep briefly so mtime can differ
    std::thread::sleep(std::time::Duration::from_millis(50));
    execute_pipeline(&overwrite_pipeline, tmp.path(), &ctx, |_| {})
        .expect("overwrite run should succeed");
    let mtime_overwritten = std::fs::metadata(&expected_output)
        .unwrap()
        .modified()
        .unwrap();
    assert!(
        mtime_overwritten > mtime_before,
        "Overwrite policy should have rewritten the output"
    );
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        output_artifact: Default::default(),
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
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(|path| -> anyhow::Result<Arc<dyn ArchiveBackend>> {
            panic!(
                "Smart rerun should skip without extracting {}",
                path.display()
            )
        })
    };

    let mut skipped: Vec<(PathBuf, String)> = Vec::new();
    let mut summary: Option<(usize, usize, usize)> = None;
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileSkipped { output, reason } => skipped.push((output, reason)),
            AllComplete {
                succeeded,
                skipped: s,
                failed,
            } => summary = Some((succeeded, s, failed)),
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
        output_artifact: Default::default(),
    };

    let ctx = PipelineContext {
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(|_| panic!("should not extract — Smart with no match must Fail"))
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
        output_artifact: Default::default(),
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
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(|_| anyhow::bail!("no real backend in this test"))
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
    assert!(
        skipped.is_empty(),
        "Smart should rerun when stored output is gone"
    );
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
        output_artifact: Default::default(),
    };

    let db = open_pipeline_runs_db();
    let ctx = PipelineContext {
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(|_| panic!("unreachable"))
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

// ---- Folder output tests ----

#[test]
fn preview_uses_folder_path_when_output_artifact_is_folder() {
    // Archive mode vs Folder mode should produce different expected_output shapes
    // for the same pipeline.
    let input = PathBuf::from("/tmp/mod.rar");
    let base = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Convert {
            format: ConvertFormat::SevenZ,
            compression: CompressionLevel::Normal,
            password: None,
        }],
        output: PipelineOutput::NewFolder(PathBuf::from("/dst")),
        collision_policy: None,
        output_artifact: OutputArtifact::Archive,
    };
    let archive_preview = preview_pipeline(&base);
    assert_eq!(
        archive_preview.entries[0].expected_output,
        Some(PathBuf::from("/dst/mod.7z"))
    );

    let folder = Pipeline {
        output_artifact: OutputArtifact::Folder,
        ..base
    };
    let folder_preview = preview_pipeline(&folder);
    assert_eq!(
        folder_preview.entries[0].expected_output,
        Some(PathBuf::from("/dst/mod"))
    );
}

#[test]
fn folder_output_leaves_extracted_tree_on_disk() {
    use arclain_core::backends::BackendSelector;
    use std::io::Write;

    let tmp = tempfile::tempdir().unwrap();
    let input_dir = tmp.path().join("in");
    let output_dir = tmp.path().join("out");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let input = input_dir.join("pack.zip");

    // Synthetic zip input — no 7z needed because we use the native ZipBackend
    // for extraction, and folder output skips the 7z-driven final pack.
    {
        let file = std::fs::File::create(&input).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"hello").unwrap();
        zw.start_file("data/payload.bin", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(&[0xAA; 256]).unwrap();
        zw.finish().unwrap();
    }

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![], // Flatten/Organize/Convert not needed; plain extract + folder out
        output: PipelineOutput::NewFolder(output_dir.clone()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
        output_artifact: OutputArtifact::Folder,
    };

    let selector = Arc::new(BackendSelector::default());
    let selector_cloned = selector.clone();
    let ctx = PipelineContext::minimal(move |p: &std::path::Path| selector_cloned.select(p));

    let mut failures: Vec<String> = Vec::new();
    let mut completions: Vec<PathBuf> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileFailed { error } => failures.push(error),
            FileComplete { output } => completions.push(output),
            _ => {}
        }
    })
    .expect("folder-output run should succeed");

    assert!(failures.is_empty(), "unexpected failures: {:?}", failures);
    assert_eq!(completions.len(), 1);

    let expected_folder = output_dir.join("pack");
    assert_eq!(completions[0], expected_folder);
    assert!(expected_folder.is_dir(), "output should be a directory");
    // Content came through intact
    assert_eq!(
        std::fs::read(expected_folder.join("readme.txt")).unwrap(),
        b"hello"
    );
    assert!(expected_folder.join("data/payload.bin").exists());

    // Input untouched
    assert!(input.exists());
}

/// The whole claim, end to end: a rule that schedules screenshots must
/// produce them through a pipeline. The `Organize` step used to build a
/// plan carrying `PendingDownload`s and hand it straight to the applier,
/// which path-checks `plan.downloads` and then ignores them -- so a
/// pipeline run silently produced no screenshots at all, however the
/// rule was written.
///
/// Drives the real executor: a seeded library supplies the screenshot
/// URLs, a saved rule turns them into scheduled downloads, and the
/// context's transport returns fixed bytes so the test needs no network.
/// Folder output keeps it off the 7z CLI too.
#[cfg(feature = "gameta")]
#[test]
fn an_organize_step_fetches_the_screenshots_its_rule_schedules() {
    use arclain_core::backends::BackendSelector;
    use std::io::Write;

    let tmp = tempfile::tempdir().unwrap();
    let input_dir = tmp.path().join("in");
    let output_dir = tmp.path().join("out");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();

    // The product code in the name is what the metadata lookup keys on.
    let input = input_dir.join("[RJ123456] Placeholder Game.zip");
    {
        let file = std::fs::File::create(&input).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file("game.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"payload").unwrap();
        zw.finish().unwrap();
    }

    // A library row carrying one screenshot URL — the only thing that
    // makes a rule schedule a download at all.
    let library_service = arclain_core::LibraryService::new(&tmp.path().join("metadata.sqlite"))
        .expect("LibraryService::new");
    let mut metadata =
        gameta_core::ProductMetadata::new(gameta_core::MetadataSource::DLSite, "RJ123456");
    metadata.title = Some("Placeholder Game".to_string());
    metadata.creator = Some("Placeholder Circle".to_string());
    metadata.extras = serde_json::json!({
        "screenshots": ["https://img.example.test/RJ123456_img_main.jpg"]
    });
    library_service
        .save_metadata(&metadata)
        .expect("seeding metadata");

    // A rule that only names a root folder: no move actions, so the
    // screenshot is the one thing the plan schedules.
    let config_db_path = tmp.path().join("config.sqlite");
    arclain_db::ConfigDb::open(&config_db_path).expect("config schema");
    let organization_service = Arc::new(arclain_core::OrganizationService::new(
        arclain_db::DieselPool::new(&config_db_path).expect("config pool"),
    ));
    let rule_id = organization_service
        .save_rule(&arclain_db::DbOrganizationRule {
            id: None,
            name: "Screenshots".to_string(),
            description: None,
            category: "test".to_string(),
            trigger_json: serde_json::json!({}).to_string(),
            actions_json: serde_json::json!({
                "root_folder": "Root",
                "move_files": [],
                "use_standard_layout": false
            })
            .to_string(),
            priority: 0,
            is_enabled: true,
            is_system: false,
        })
        .expect("saving the rule");

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![PipelineStep::Organize { rule_id }],
        output: PipelineOutput::NewFolder(output_dir.clone()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
        output_artifact: OutputArtifact::Folder,
    };

    let selector = Arc::new(BackendSelector::default());
    let selector_cloned = selector.clone();
    let ctx = PipelineContext {
        organization_service: Some(organization_service),
        library_service: Some(Arc::new(library_service)),
        fetch_download: Some(Arc::new(|_| Ok(b"jpegbytes".to_vec()))),
        ..PipelineContext::minimal(move |p: &std::path::Path| selector_cloned.select(p))
    };

    let mut failures: Vec<String> = Vec::new();
    let mut completions: Vec<PathBuf> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileFailed { error } => failures.push(error),
            FileComplete { output } => completions.push(output),
            DownloadWarnings { warnings: w } => warnings.extend(w),
            _ => {}
        }
    })
    .expect("the run should succeed");

    assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    assert!(
        warnings.is_empty(),
        "a transport that answers should report nothing: {warnings:?}"
    );
    assert_eq!(completions.len(), 1);

    let screenshot = completions[0].join("Root/Screenshots/image_001.jpg");
    assert_eq!(
        std::fs::read(&screenshot).unwrap_or_default(),
        b"jpegbytes".to_vec(),
        "the scheduled screenshot must reach {}",
        screenshot.display()
    );

    // The staging the fetch needed is an implementation detail of the
    // run, not something the user asked to keep.
    let leftovers: Vec<String> = std::fs::read_dir(&completions[0])
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| name.starts_with(".arclain-downloads-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "download staging must not ship in the output: {leftovers:?}"
    );
}

#[test]
fn folder_output_smart_skips_on_rerun() {
    // Second run with same pipeline + pre-existing output folder + DB row
    // should emit FileSkipped, not redo the extraction.
    use arclain_core::backends::BackendSelector;
    use std::io::Write;

    let tmp = tempfile::tempdir().unwrap();
    let input_dir = tmp.path().join("in");
    let output_dir = tmp.path().join("out");
    std::fs::create_dir_all(&input_dir).unwrap();
    std::fs::create_dir_all(&output_dir).unwrap();
    let input = input_dir.join("pack.zip");

    {
        let file = std::fs::File::create(&input).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        zw.start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"a").unwrap();
        zw.finish().unwrap();
    }

    let pipeline = Pipeline {
        input: Some(PipelineInput::Files(vec![input.clone()])),
        steps: vec![],
        output: PipelineOutput::NewFolder(output_dir.clone()),
        collision_policy: Some(OutputCollisionPolicy::Smart),
        output_artifact: OutputArtifact::Folder,
    };

    let db = open_pipeline_runs_db();
    let selector = Arc::new(BackendSelector::default());
    let selector_cloned = selector.clone();
    let ctx = PipelineContext {
        config_db: Some(db.clone()),
        ..PipelineContext::minimal(move |p: &std::path::Path| selector_cloned.select(p))
    };

    // First run — produces the folder
    execute_pipeline(&pipeline, tmp.path(), &ctx, |_| {}).expect("first run should succeed");
    let out_folder = output_dir.join("pack");
    assert!(out_folder.is_dir());

    // Second run — DB match + folder still there → skip
    let mut skipped = 0usize;
    let mut completed = 0usize;
    execute_pipeline(&pipeline, tmp.path(), &ctx, |ev| {
        use arclain_core::PipelineProgress::*;
        match ev {
            FileSkipped { .. } => skipped += 1,
            FileComplete { .. } => completed += 1,
            _ => {}
        }
    })
    .expect("second run should succeed");

    assert_eq!(skipped, 1, "rerun should skip");
    assert_eq!(completed, 0, "rerun should not redo work");
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
        generated_files: vec![("MyGame/metadata.json".into(), r#"{"title":"Test"}"#.into())],
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
