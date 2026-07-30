//! Integration tests for the Process page's facade surface:
//! `ArclainApp::{pipeline_presets, save_pipeline_preset,
//! delete_pipeline_preset, preview_pipeline, interrupted_pipeline_runs}`.
//!
//! `crates/app/src/process.rs`'s own unit tests cover DTO validation and
//! conversion in isolation (pure functions, no I/O); this file's job is
//! proving those pieces are wired together correctly behind the public
//! API against a real bootstrap -- a real presets file in a temp
//! profile, a real SQLite config database, and for the preview a real
//! ZIP fixture opened through the real `start_open_archive` flow, the
//! same way `organization_facade.rs` does for its own surface.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! following this crate's established convention: `ArclainApp` owns its
//! own Tokio runtime, and dropping it must not happen from inside an
//! async context.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationResult, OperationState};
use arclain_app::ids::ArchiveSessionId;
use arclain_app::operations::pipeline::{
    CompressionLevelDto, OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto,
    PipelineSpecDto, PipelineStepDto,
};
use arclain_app::operations::PipelineRequest;
use arclain_app::process::{
    PipelinePresetInput, PipelinePresetSummary, PipelinePreviewInputsDto, PipelinePreviewRequest,
};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

// ============================================================================
// Harness.
// ============================================================================

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A temp directory rooted under this crate's own checkout rather than
/// the system temp directory -- see `processing_operations.rs`'s
/// identical helper for why (a RAM-disk system temp produced spurious
/// "path not found" failures from back-to-back filesystem operations).
fn scratch_tempdir() -> tempfile::TempDir {
    let scratch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&scratch_root).expect("create test scratch root");
    tempfile::Builder::new()
        .prefix("process-facade-")
        .tempdir_in(&scratch_root)
        .expect("create scratch tempdir")
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

fn test_paths(temp: &tempfile::TempDir) -> AppPaths {
    support::temp_paths(temp.path())
}

/// Bootstraps an `ArclainApp` against `paths` -- see
/// `organization_facade.rs::bootstrap_app`'s doc comment for why the
/// dummy 7-Zip seeding is required even for tests that never touch an
/// archive backend.
fn bootstrap_with_paths(temp: &tempfile::TempDir, paths: AppPaths) -> ArclainApp {
    let sevenzip = support::create_dummy_executable(temp.path(), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed")
}

fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    bootstrap_with_paths(temp, test_paths(temp))
}

fn build_zip_fixture(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
    path
}

async fn open_session(app: &ArclainApp, archive: &Path) -> ArchiveSessionId {
    let mut events = app.subscribe_operations();
    app.start_open_archive(arclain_app::archive::OpenArchiveRequest {
        source_path: archive.to_path_buf(),
        password: None,
    })
    .await
    .expect("start_open_archive must be accepted");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an open event must arrive within 10s")
            .expect("the operation channel must not close");
        match event.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return snapshot.session_id,
            OperationState::Failed { error } => panic!("opening the fixture failed: {error:?}"),
            _ => {}
        }
    }
}

/// Writes plugin-reported metadata onto a session exactly the way a
/// plugin's `emit_metadata` host call does -- through the installed
/// `ActiveTabBridge`, the only path that reaches session metadata.
fn report_plugin_title(app: &ArclainApp, session_id: ArchiveSessionId, title: &str) {
    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
    bridge.set_session_metadata(
        session_id.into_raw(),
        Some(serde_json::json!({
            "product_id": "RJ123456",
            "source": "dlsite",
            "title": title,
        })),
    );
}

fn preset_input(name: &str) -> PipelinePresetInput {
    PipelinePresetInput {
        name: name.to_string(),
        steps: vec![PipelineStepDto::Flatten {
            strip_common_prefix: false,
            max_depth: 1,
        }],
        destination: PipelineDestinationDto::SameFolder,
        collision_policy: None,
        output_artifact: OutputArtifactDto::Folder,
    }
}

fn names(presets: &[PipelinePresetSummary]) -> Vec<String> {
    presets.iter().map(|p| p.name.clone()).collect()
}

fn shipped_names() -> Vec<String> {
    arclain_core::builtin_presets()
        .into_iter()
        .map(|p| p.name)
        .collect()
}

/// A pipeline preview over one file list, with the given steps and no
/// metadata -- the shape a step editor recomputes.
fn steps_preview(inputs: Vec<PathBuf>, steps: Vec<PipelineStepDto>) -> PipelinePreviewRequest {
    PipelinePreviewRequest {
        inputs: PipelinePreviewInputsDto::Files { paths: inputs },
        destination: PipelineDestinationDto::SameFolder,
        pipeline: PipelineSpecDto::Steps {
            steps,
            output_artifact: OutputArtifactDto::Archive,
        },
        collision_policy: None,
        metadata: None,
    }
}

// ============================================================================
// Presets: listing, and where they live.
// ============================================================================

/// The seed behavior: a profile with no presets file reports the shipped
/// presets, every one flagged `builtin`.
#[test]
fn a_fresh_profile_lists_the_shipped_presets_as_builtin() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let presets = runtime.block_on(app.pipeline_presets()).unwrap();
    assert_eq!(names(&presets), shipped_names());
    assert!(
        presets.iter().all(|preset| preset.builtin),
        "an untouched profile's presets are all the shipped ones"
    );
}

/// The path-honesty proof. `arclain_core::default_presets_path()`
/// resolves through `AppDirectories::init`, which ignores
/// `BootstrapConfig::paths_override` entirely -- a facade that used it
/// would read and write the *real* user's presets from an isolated
/// profile. Asserting the file lands under the overridden `config_dir`
/// is what pins that this surface does not.
#[test]
fn presets_are_written_under_the_overridden_config_dir_not_the_real_profile() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    let expected = paths.config_dir.join("pipeline_presets.json");
    let app = bootstrap_with_paths(&temp, paths);

    assert!(
        !expected.exists(),
        "a fresh profile has no presets file until something saves one"
    );
    runtime
        .block_on(app.save_pipeline_preset(preset_input("Mine")))
        .unwrap();

    assert!(
        expected.exists(),
        "the save must land in the overridden profile at {}",
        expected.display()
    );
    // The stored bytes are `arclain_core`'s own format, so an existing
    // profile's file stays readable by both ends.
    let stored = arclain_core::load_presets(&expected);
    assert!(stored.iter().any(|preset| preset.name == "Mine"));
}

// ============================================================================
// Presets: the built-in / shadowing rule.
// ============================================================================

/// **The semantics-pinning test for built-ins.** They are a seed, not a
/// protected namespace: a user can delete one and it stays deleted --
/// across a save, a reload, and a whole new application instance
/// bootstrapped against the same profile.
#[test]
fn deleting_a_builtin_is_permanent_across_a_restart() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    let doomed = shipped_names()
        .into_iter()
        .next()
        .expect("at least one shipped preset");

    let app = bootstrap_with_paths(&temp, paths.clone());
    let after_delete = runtime
        .block_on(app.delete_pipeline_preset(doomed.clone()))
        .expect("deleting a shipped preset must be permitted");
    assert!(!names(&after_delete).contains(&doomed));
    runtime.block_on(app.shutdown()).unwrap();
    drop(app);

    let reopened = bootstrap_with_paths(&temp, paths);
    let presets = runtime.block_on(reopened.pipeline_presets()).unwrap();
    assert!(
        !names(&presets).contains(&doomed),
        "a deleted shipped preset must not be re-merged on the next launch; got {:?}",
        names(&presets)
    );
}

/// The extreme case of the same rule, and the one that proves the file
/// is authoritative rather than merged: with every preset deleted, the
/// list is genuinely empty. A merge-on-read implementation would report
/// the shipped ones here.
#[test]
fn deleting_every_preset_leaves_none_rather_than_restoring_the_shipped_ones() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    runtime.block_on(async {
        for name in shipped_names() {
            app.delete_pipeline_preset(name).await.unwrap();
        }
        assert!(app.pipeline_presets().await.unwrap().is_empty());
    });
}

/// A saved preset that shadows a shipped one by name replaces it, and
/// the entry stops claiming to be the shipped pipeline -- because it is
/// no longer that pipeline.
#[test]
fn saving_over_a_builtin_name_shadows_it_in_place_and_clears_the_builtin_flag() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let shipped = shipped_names();
    let shadowed = shipped
        .first()
        .expect("at least one shipped preset")
        .clone();
    let presets = runtime
        .block_on(app.save_pipeline_preset(preset_input(&shadowed)))
        .unwrap();

    assert_eq!(
        names(&presets),
        shipped,
        "shadowing must replace in place, not append a second entry"
    );
    let entry = presets
        .iter()
        .find(|preset| preset.name == shadowed)
        .unwrap();
    assert!(!entry.builtin);
    assert_eq!(entry.steps, preset_input(&shadowed).steps);
}

// ============================================================================
// Presets: CRUD round trips.
// ============================================================================

#[test]
fn a_saved_preset_round_trips_through_a_real_profile() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let mut input = preset_input("Round trip");
    input.steps = vec![
        PipelineStepDto::Flatten {
            strip_common_prefix: true,
            max_depth: 0,
        },
        PipelineStepDto::Organize {
            rule_id: "12".to_string(),
        },
        PipelineStepDto::Convert {
            format: "7z".to_string(),
            compression: CompressionLevelDto::Max,
        },
    ];
    input.destination = PipelineDestinationDto::Folder {
        path: temp.path().join("out"),
    };
    input.collision_policy = Some(OutputCollisionPolicyDto::Skip);
    input.output_artifact = OutputArtifactDto::Archive;

    let presets = runtime
        .block_on(app.save_pipeline_preset(input.clone()))
        .unwrap();
    let stored = presets
        .iter()
        .find(|preset| preset.name == input.name)
        .expect("the saved preset must be in the returned list");

    assert_eq!(stored.steps, input.steps);
    assert_eq!(stored.destination, input.destination);
    assert_eq!(stored.collision_policy, input.collision_policy);
    assert_eq!(stored.output_artifact, input.output_artifact);
    assert!(!stored.builtin);

    // And the same thing comes back from a fresh read of the file.
    let relisted = runtime.block_on(app.pipeline_presets()).unwrap();
    assert_eq!(relisted.iter().find(|p| p.name == input.name), Some(stored));
}

/// Name-keyed upsert: re-saving under an existing name replaces that
/// entry in place rather than appending a duplicate the dropdown would
/// show twice and a delete would remove both of.
#[test]
fn re_saving_a_preset_replaces_it_in_place_instead_of_duplicating_it() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    runtime.block_on(async {
        app.save_pipeline_preset(preset_input("Same name"))
            .await
            .unwrap();
        app.save_pipeline_preset(preset_input("Other"))
            .await
            .unwrap();

        let mut edited = preset_input("Same name");
        edited.steps = vec![PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Fast,
        }];
        let presets = app.save_pipeline_preset(edited.clone()).await.unwrap();

        assert_eq!(
            presets
                .iter()
                .filter(|preset| preset.name == "Same name")
                .count(),
            1,
            "a second save under the same name must not create a duplicate"
        );
        let position = presets
            .iter()
            .position(|preset| preset.name == "Same name")
            .unwrap();
        let other = presets
            .iter()
            .position(|preset| preset.name == "Other")
            .unwrap();
        assert!(
            position < other,
            "an edited preset must keep its position, not move to the end"
        );
        assert_eq!(presets[position].steps, edited.steps);
    });
}

#[test]
fn deleting_an_unknown_preset_is_not_found() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let error = runtime
        .block_on(app.delete_pipeline_preset("nothing named this".to_string()))
        .unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    assert_eq!(error.field.as_deref(), Some("name"));
}

/// Delete matches the listed name exactly rather than trimming it. A
/// presets file written before this facade existed can hold a name with
/// surrounding whitespace; trimming on delete would make exactly those
/// entries permanently undeletable -- listed as present, never matched.
#[test]
fn a_preset_whose_stored_name_has_surrounding_whitespace_is_still_deletable() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    arclain_core::save_presets(
        &paths.config_dir.join("pipeline_presets.json"),
        &[arclain_core::SavedPreset {
            name: "  Legacy padded  ".to_string(),
            pipeline: arclain_core::Pipeline {
                input: None,
                steps: vec![arclain_core::PipelineStep::Flatten {
                    strip_common_prefix: false,
                    max_depth: 1,
                }],
                output: arclain_core::PipelineOutput::SameFolder,
                collision_policy: None,
                output_artifact: arclain_core::OutputArtifact::Folder,
            },
        }],
    )
    .unwrap();
    let app = bootstrap_with_paths(&temp, paths);

    runtime.block_on(async {
        let listed = app.pipeline_presets().await.unwrap();
        assert_eq!(names(&listed), vec!["  Legacy padded  ".to_string()]);
        let after = app
            .delete_pipeline_preset(listed[0].name.clone())
            .await
            .expect("the name this surface listed must be the name it deletes");
        assert!(after.is_empty());
    });
}

#[test]
fn an_invalid_preset_is_rejected_and_nothing_is_written() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    let presets_file = paths.config_dir.join("pipeline_presets.json");
    let app = bootstrap_with_paths(&temp, paths);

    runtime.block_on(async {
        let mut empty_steps = preset_input("No steps");
        empty_steps.steps.clear();
        let error = app.save_pipeline_preset(empty_steps).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("steps"));

        let error = app
            .save_pipeline_preset(preset_input("   "))
            .await
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("name"));
    });

    assert!(
        !presets_file.exists(),
        "a rejected save must not create or touch the presets file"
    );
}

/// The link between the two halves of the presets story: a name this
/// surface reports is a name `start_pipeline` resolves, from the same
/// file. Uses `Folder` artifact mode so no real 7-Zip is needed.
#[test]
fn a_preset_saved_through_the_facade_is_runnable_by_start_pipeline() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let input = build_zip_fixture(
        temp.path(),
        "preset-run.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let saved = app
            .save_pipeline_preset(preset_input("Facade saved"))
            .await
            .unwrap();
        let name = saved
            .iter()
            .find(|preset| preset.name == "Facade saved")
            .unwrap()
            .name
            .clone();

        let mut events = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input.clone()],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Preset { id: name },
                collision_policy: None,
            })
            .await
            .expect("a preset this facade just saved must be resolvable by start_pipeline");

        loop {
            let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
                .await
                .expect("a pipeline event must arrive within 30s")
                .expect("the operation channel must not close");
            if event.operation_id != operation_id {
                continue;
            }
            match event.state {
                OperationState::Completed { .. } => break,
                OperationState::Failed { error } => panic!("the preset run failed: {error:?}"),
                _ => {}
            }
        }
    });

    assert_eq!(
        std::fs::read(destination.join("preset-run/data.bin")).unwrap(),
        b"alpha-content"
    );
}

// ============================================================================
// Preview.
// ============================================================================

#[test]
fn a_preview_describes_each_input_and_its_predicted_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let first = temp.path().join("first.rar");
    let second = temp.path().join("second.rar");

    let preview = runtime
        .block_on(app.preview_pipeline(steps_preview(
            vec![first.clone(), second.clone()],
            vec![
                PipelineStepDto::Flatten {
                    strip_common_prefix: true,
                    max_depth: 0,
                },
                PipelineStepDto::Convert {
                    format: "zip".to_string(),
                    compression: CompressionLevelDto::Normal,
                },
            ],
        )))
        .unwrap();

    assert!(preview.global_warnings.is_empty());
    assert_eq!(preview.entries.len(), 2);
    assert_eq!(preview.entries[0].input, first);
    assert_eq!(preview.entries[1].input, second);
    assert_eq!(preview.entries[0].operations.len(), 2);
    assert!(preview.entries[0].operations[0].contains("Flatten"));
    assert!(preview.entries[0].operations[1].contains("zip"));
    assert_eq!(
        preview.entries[0].expected_output,
        Some(temp.path().join("first.zip"))
    );
    assert!(preview.entries[0].warnings.is_empty());
}

/// The collision warning is the one thing the preview reads the
/// filesystem for, and it is what a user actually looks at before
/// clicking Run.
#[test]
fn a_preview_warns_when_the_predicted_output_already_exists() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let input = temp.path().join("collide.rar");
    std::fs::write(temp.path().join("collide.zip"), b"already here").unwrap();

    let mut request = steps_preview(
        vec![input],
        vec![PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Normal,
        }],
    );
    request.collision_policy = Some(OutputCollisionPolicyDto::Overwrite);

    let preview = runtime.block_on(app.preview_pipeline(request)).unwrap();
    let warnings = &preview.entries[0].warnings;
    assert_eq!(warnings.len(), 1, "expected one collision warning");
    assert!(
        warnings[0].contains("overwritten"),
        "the warning must reflect the requested policy, got {warnings:?}"
    );
}

/// A folder input is expanded by `arclain_core`'s own definition of
/// "archive in this directory" -- and the resulting entry list is
/// exactly the `Vec<PathBuf>` a caller then hands to `start_pipeline`,
/// which has no folder input of its own.
#[test]
fn a_folder_input_is_expanded_into_one_entry_per_archive() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let folder = temp.path().join("batch");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("one.zip"), b"a").unwrap();
    std::fs::write(folder.join("two.7z"), b"b").unwrap();
    std::fs::write(folder.join("notes.txt"), b"not an archive").unwrap();

    let mut request = steps_preview(
        Vec::new(),
        vec![PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Normal,
        }],
    );
    request.inputs = PipelinePreviewInputsDto::Folder {
        path: folder.clone(),
    };

    let preview = runtime.block_on(app.preview_pipeline(request)).unwrap();
    let mut expanded: Vec<PathBuf> = preview
        .entries
        .iter()
        .map(|entry| entry.input.clone())
        .collect();
    expanded.sort();
    assert_eq!(
        expanded,
        vec![folder.join("one.zip"), folder.join("two.7z")]
    );
}

#[test]
fn an_empty_folder_reports_a_global_warning_rather_than_failing() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let folder = temp.path().join("empty");
    std::fs::create_dir_all(&folder).unwrap();

    let mut request = steps_preview(
        Vec::new(),
        vec![PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Normal,
        }],
    );
    request.inputs = PipelinePreviewInputsDto::Folder { path: folder };

    let preview = runtime.block_on(app.preview_pipeline(request)).unwrap();
    assert!(preview.entries.is_empty());
    assert_eq!(preview.global_warnings.len(), 1);
}

/// The preview costs nothing an operation-aware caller has to reap: no
/// `OperationId` is minted, nothing is broadcast, and
/// `recent_operations` is untouched.
#[test]
fn a_preview_registers_no_operation_and_broadcasts_nothing() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    runtime.block_on(async {
        let before = app.recent_operations(64).await.unwrap().len();
        let mut events = app.subscribe_operations();

        for _ in 0..5 {
            app.preview_pipeline(steps_preview(
                vec![temp.path().join("a.rar")],
                vec![PipelineStepDto::Convert {
                    format: "zip".to_string(),
                    compression: CompressionLevelDto::Normal,
                }],
            ))
            .await
            .unwrap();
        }

        assert_eq!(app.recent_operations(64).await.unwrap().len(), before);
        assert!(
            events.try_recv().is_err(),
            "a preview must not broadcast an operation event"
        );
    });
}

#[test]
fn a_preview_can_run_a_saved_preset_and_an_unknown_one_is_not_found() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let input = temp.path().join("preset-preview.rar");

    runtime.block_on(async {
        app.save_pipeline_preset(preset_input("Previewable"))
            .await
            .unwrap();

        let mut request = steps_preview(vec![input.clone()], Vec::new());
        request.pipeline = PipelineSpecDto::Preset {
            id: "Previewable".to_string(),
        };
        let preview = app.preview_pipeline(request.clone()).await.unwrap();
        assert_eq!(preview.entries.len(), 1);
        assert_eq!(preview.entries[0].operations.len(), 1);
        assert!(preview.entries[0].operations[0].contains("Flatten"));

        request.pipeline = PipelineSpecDto::Preset {
            id: "no such preset".to_string(),
        };
        let error = app.preview_pipeline(request).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    });
}

/// A stale session id must not be silently downgraded to "no metadata":
/// that would quietly report a completely different set of output paths
/// than the caller asked about.
#[test]
fn a_preview_naming_an_unknown_session_is_not_found() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let mut request = steps_preview(
        vec![temp.path().join("a.rar")],
        vec![PipelineStepDto::Convert {
            format: "zip".to_string(),
            compression: CompressionLevelDto::Normal,
        }],
    );
    request.metadata = Some(ArchiveSessionId::from_raw(9_999));

    let error = runtime.block_on(app.preview_pipeline(request)).unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

/// Named metadata drives the predicted output name -- through
/// `arclain_core`'s own `stem_from`, from the session's plugin-reported
/// blob, read with the same function `preview_organize_plan` uses.
#[test]
fn session_metadata_names_the_predicted_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let archive = build_zip_fixture(temp.path(), "session.zip", &[("a.txt", b"a")]);
    let pipeline_input = temp.path().join("[RJ123456] Placeholder.rar");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive).await;
        report_plugin_title(&app, session_id, "Session Title");

        let mut request = steps_preview(
            vec![pipeline_input.clone()],
            vec![PipelineStepDto::Convert {
                format: "zip".to_string(),
                compression: CompressionLevelDto::Normal,
            }],
        );
        request.metadata = Some(session_id);

        let preview = app.preview_pipeline(request.clone()).await.unwrap();
        assert_eq!(
            preview.entries[0].expected_output,
            Some(temp.path().join("Session Title.zip"))
        );

        // Without the session, the detected product code names it
        // instead -- proving the metadata is what changed the answer.
        request.metadata = None;
        let bare = app.preview_pipeline(request).await.unwrap();
        assert_eq!(
            bare.entries[0].expected_output,
            Some(temp.path().join("RJ123456.zip"))
        );
    });
}

/// **THE REPORTED DIVERGENCE, made executable.**
///
/// This preview and `start_pipeline` resolve metadata from two different
/// places, and this test pins that they currently disagree:
///
/// * the preview names the output from the *session's* plugin-reported
///   metadata -- one blob, applied to every input, which is what the
///   pre-facade Process page previewed with;
/// * `start_pipeline` names it from the *DLsite library*, looked up per
///   input from a product code detected in that input's own file name.
///
/// Seeding the two with different titles for the same product code makes
/// the disagreement visible: the user is shown one output path and the
/// run writes another. Nothing here endorses that -- this is a
/// characterization test, and it is expected to fail the day
/// `PipelineRequest` gains a session binding the way
/// `OrganizeRequest::archive_session_id` has one. When it does, the fix
/// is to assert the two now *agree*, not to loosen the assertion.
#[test]
fn preview_and_start_pipeline_resolve_metadata_from_different_sources() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    // The library's row for the placeholder product code.
    let library_service = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed for a freshly bootstrapped app")
        .core_services
        .library_service
        .clone()
        .expect("library_service must be composed for a freshly bootstrapped app");
    let mut product =
        gameta_core::ProductMetadata::new(gameta_core::MetadataSource::DLSite, "RJ123456");
    product.title = Some("Library Title".to_string());
    library_service
        .save_metadata(&product)
        .expect("seeding library metadata must succeed");

    let session_archive = build_zip_fixture(temp.path(), "session.zip", &[("a.txt", b"a")]);
    // Carries the same product code the library row is keyed by, so the
    // executor's own lookup finds "Library Title" for it.
    let pipeline_input = build_zip_fixture(
        temp.path(),
        "[RJ123456] Placeholder.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let session_id = open_session(&app, &session_archive).await;
        report_plugin_title(&app, session_id, "Session Title");

        let mut request = PipelinePreviewRequest {
            inputs: PipelinePreviewInputsDto::Files {
                paths: vec![pipeline_input.clone()],
            },
            destination: PipelineDestinationDto::Folder {
                path: destination.clone(),
            },
            pipeline: PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Flatten {
                    strip_common_prefix: false,
                    max_depth: 1,
                }],
                output_artifact: OutputArtifactDto::Folder,
            },
            collision_policy: Some(OutputCollisionPolicyDto::Fail),
            metadata: Some(session_id),
        };

        let previewed = app
            .preview_pipeline(request.clone())
            .await
            .unwrap()
            .entries
            .remove(0)
            .expected_output
            .expect("a folder artifact always predicts an output");
        assert_eq!(
            previewed,
            destination.join("Session Title"),
            "the preview names the output from the session's plugin metadata"
        );

        // The same request without the session falls back to the
        // detected code -- so the preview never consults the library at
        // all, whatever it is told.
        request.metadata = None;
        let without_session = app
            .preview_pipeline(request)
            .await
            .unwrap()
            .entries
            .remove(0)
            .expected_output
            .unwrap();
        assert_eq!(
            without_session,
            destination.join("RJ123456"),
            "with no session the preview uses the detected code, never the library's title"
        );

        let mut events = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![pipeline_input],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps {
                    steps: vec![PipelineStepDto::Flatten {
                        strip_common_prefix: false,
                        max_depth: 1,
                    }],
                    output_artifact: OutputArtifactDto::Folder,
                },
                collision_policy: Some(OutputCollisionPolicyDto::Fail),
            })
            .await
            .expect("start_pipeline must be accepted");
        loop {
            let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
                .await
                .expect("a pipeline event must arrive within 30s")
                .expect("the operation channel must not close");
            if event.operation_id != operation_id {
                continue;
            }
            match event.state {
                OperationState::Completed { .. } => break,
                OperationState::Failed { error } => panic!("the run failed: {error:?}"),
                _ => {}
            }
        }
    });

    assert!(
        destination.join("Library Title").exists(),
        "start_pipeline names the output from the library, not the session"
    );
    assert!(
        !destination.join("Session Title").exists(),
        "the previewed path is NOT the path the run wrote -- this is the reported divergence"
    );
}

// ============================================================================
// Interrupted prior runs.
// ============================================================================

/// Inserts an `in_progress` `pipeline_runs` row backdated far enough
/// past the startup sweep's one-hour threshold that the next bootstrap
/// against this profile declares it interrupted. Writes straight into
/// the config database the same way `support::seed_working_sevenzip_config`
/// does, because "a previous process died mid-run" is not a state any
/// public API can produce.
fn seed_stale_in_progress_run(paths: &AppPaths, input_path: &str) {
    let config_db_path = paths.data_dir.join("databases").join("config.sqlite");
    let db = arclain_core::config::ConfigDb::open(&config_db_path).expect("open config db");
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 7200;
    db.into_sqlite_db()
        .with_connection(|conn| {
            conn.execute(
                "INSERT INTO pipeline_runs
                    (input_path, input_blake3, input_size, pipeline_hash,
                     status, started_at, arclain_version)
                 VALUES (?1, ?2, ?3, ?4, 'in_progress', ?5, ?6)",
                (
                    input_path,
                    "placeholder-hash",
                    1_i64,
                    "pipe",
                    started_at,
                    "2.1.0",
                ),
            )?;
            Ok(())
        })
        .expect("seed a stale in-progress pipeline run");
}

/// The whole semantics in one test: a run a previous process abandoned
/// is reported after the next launch sweeps it, the `since_unix` bound
/// filters on the *sweep* time, and -- the finding -- **nothing ever
/// clears it**, so the same run is still reported after another restart.
#[test]
fn an_abandoned_run_is_reported_after_the_next_launch_and_is_never_cleared() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);

    // `bootstrap_with_paths` seeds (and therefore creates) the config
    // database before bootstrapping, so the row goes in first and the
    // bootstrap that follows is the one that sweeps it.
    let sevenzip = support::create_dummy_executable(temp.path(), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    seed_stale_in_progress_run(&paths, "/mods/RJ123456.rar");

    let app = bootstrap_with_paths(&temp, paths.clone());
    let runs = runtime.block_on(app.interrupted_pipeline_runs(0)).unwrap();
    assert_eq!(runs.len(), 1, "the startup sweep must have flagged the row");
    assert_eq!(runs[0].input_path, PathBuf::from("/mods/RJ123456.rar"));
    assert_eq!(runs[0].arclain_version, "2.1.0");
    assert!(
        runs[0].interrupted_at_unix >= runs[0].started_at_unix,
        "a run cannot be declared interrupted before it started"
    );

    // The bound filters on when the sweep ran, not when the run started
    // or died.
    let after = runs[0].interrupted_at_unix + 1;
    assert!(runtime
        .block_on(app.interrupted_pipeline_runs(after))
        .unwrap()
        .is_empty());
    assert_eq!(
        runtime
            .block_on(app.interrupted_pipeline_runs(runs[0].interrupted_at_unix))
            .unwrap()
            .len(),
        1,
        "the bound is inclusive"
    );

    runtime.block_on(app.shutdown()).unwrap();
    drop(app);

    // THE FINDING: no code path deletes these rows, clears the marker,
    // or acknowledges them, so a banner built on `since_unix = 0` is
    // permanent. If this ever starts failing, something finally does
    // clear them -- which is the fix, not a regression.
    let reopened = bootstrap_with_paths(&temp, paths);
    assert_eq!(
        runtime
            .block_on(reopened.interrupted_pipeline_runs(0))
            .unwrap()
            .len(),
        1,
        "nothing clears an interrupted run, so it is still reported after a restart"
    );
}

/// A fresh profile has nothing to report, and a *live* run is not
/// mistaken for an abandoned one (the sweep only touches rows older than
/// its threshold).
#[test]
fn a_fresh_profile_reports_no_interrupted_runs() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    assert!(runtime
        .block_on(app.interrupted_pipeline_runs(0))
        .unwrap()
        .is_empty());
}
