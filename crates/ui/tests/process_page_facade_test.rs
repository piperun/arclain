//! The Process page against a real application facade.
//!
//! The page is a render function that emits intents plus a dispatcher
//! (`features::process::view::handle_process_action`) that runs them, so
//! these drive the dispatcher directly -- the same pattern
//! `organize_panel_test.rs` and `dispatcher_test.rs` use -- and assert
//! on what the page and its run signal then hold. What matters here is
//! not pixels but four contracts:
//!
//! * the page runs *exactly* the pipeline it previewed (one request,
//!   converted, never re-derived),
//! * a folder input is still a folder when the run starts, so files
//!   added after the preview are picked up,
//! * the page's Cancel reaches the operation registry,
//! * presets and the interrupted-run banner are the application's
//!   answers, not the page's own file/database reads.

mod common;
use common::create_test_shared_state_with_facade;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arclain_app::operations::pipeline::{
    OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto, PipelineInputsDto,
    PipelineStepDto,
};
use arclain_ui::core::operations::process_runner;
use arclain_ui::features::process::view::{handle_process_action, ProcessAction};
use arclain_ui::features::process::ProcessPageState;
use arclain_ui::shared::SharedState;

/// Builds a real ZIP at `dir/name`; the pipeline reads it through the
/// application's own backend selection, so no override is needed.
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

/// A page whose caches are already seated, so a test drives only the
/// action it is actually about.
fn seeded_page() -> ProcessPageState {
    let mut state = ProcessPageState::default();
    state.interrupted_run_count = Some(0);
    state.cached_org_rules = Some(Vec::new());
    state.presets = Some(Vec::new());
    state
}

/// A Flatten-only, folder-output pipeline: real work with no dependency
/// on a 7-Zip CLI being installed (an `Archive` artifact packs through
/// one; a `Folder` artifact does not).
fn flatten_to_folder(state: &mut ProcessPageState, inputs: PipelineInputsDto, destination: &Path) {
    state.draft.inputs = inputs;
    state.draft.destination = PipelineDestinationDto::Folder {
        path: destination.to_path_buf(),
    };
    state.draft.output_artifact = OutputArtifactDto::Folder;
    state.draft.collision_policy = Some(OutputCollisionPolicyDto::Fail);
    state.draft.steps = vec![PipelineStepDto::Flatten {
        strip_common_prefix: false,
        max_depth: 1,
    }];
    state.mark_dirty();
}

/// Spins the run signal until the run reports a terminal state.
fn wait_for_run_to_finish(shared: &SharedState) -> arclain_ui::core::signals::ProcessRunState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let run = shared.signals().process_run.get();
        if run.completed {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "the pipeline run never reached a terminal state; last message: {:?}",
            run.message
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Spins until the page's run has an operation id (the dispatch is
/// asynchronous, so the id lands a moment after `RunPipeline` returns).
fn wait_for_operation_id(shared: &SharedState) -> arclain_app::ids::OperationId {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(id) = shared.signals().process_run.get().operation_id {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "the run never registered an operation"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ── preview → run ───────────────────────────────────────────────────────

/// **The invariant this page exists to keep: what it shows is what it
/// runs.**
///
/// Pre-facade the page previewed through `arclain_core::
/// preview_pipeline_with_metadata` (with the active tab's plugin
/// metadata folded in) and *separately* built an `arclain_core::Pipeline`
/// for `execute_pipeline` — two descriptions of one run, each free to
/// drift. It now previews one `PipelinePreviewRequest` and runs the
/// `PipelineRequest` that request converts into. This pins the
/// consequence end to end: the path the panel displayed is the path the
/// run wrote.
#[test]
fn the_page_runs_exactly_the_pipeline_it_previewed() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let mut state = seeded_page();

    let input = build_zip_fixture(
        temp.path(),
        "RJ123456 placeholder.zip",
        &[("nested/data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");
    flatten_to_folder(
        &mut state,
        PipelineInputsDto::Files {
            paths: vec![input.clone()],
        },
        &destination,
    );

    handle_process_action(&mut state, ProcessAction::RefreshPreview, &shared);
    assert!(
        !state.preview_dirty,
        "a completed preview must clear the dirty flag"
    );
    assert_eq!(state.preview.entries.len(), 1);
    let predicted = state.preview.entries[0]
        .expected_output
        .clone()
        .expect("a folder artifact always predicts an output");

    handle_process_action(&mut state, ProcessAction::RunPipeline, &shared);
    let run = wait_for_run_to_finish(&shared);

    assert!(!run.cancelled, "the run must not report itself cancelled");
    assert!(
        predicted.exists(),
        "the run must write exactly the path the preview predicted ({}); \
         run log: {:?}",
        predicted.display(),
        run.log
    );
    // Named from the product code detected in the input's own file name
    // (no library row was seeded, so there is no title to prefer) --
    // spelled out so a change in the naming ladder shows up here as a
    // changed name rather than as a still-passing tautology.
    assert_eq!(predicted, destination.join("RJ123456"));
    assert!(
        run.operation_id.is_none(),
        "a finished run must not leave a stale operation id behind for a cancel to hit"
    );
}

/// A folder input stays a folder all the way to the run, so an archive
/// dropped in after the preview is still processed. Pre-expanding it in
/// the page (previewing a folder and handing the resulting file list to
/// the run) would silently drop exactly this file.
#[test]
fn a_folder_input_is_expanded_at_run_time_not_at_preview_time() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let mut state = seeded_page();

    let source = temp.path().join("inbox");
    std::fs::create_dir_all(&source).unwrap();
    build_zip_fixture(&source, "first.zip", &[("nested/a.bin", b"a")]);

    let destination = temp.path().join("out");
    flatten_to_folder(
        &mut state,
        PipelineInputsDto::Folder {
            path: source.clone(),
        },
        &destination,
    );

    handle_process_action(&mut state, ProcessAction::RefreshPreview, &shared);
    assert_eq!(
        state.preview.entries.len(),
        1,
        "the preview describes the folder as it is right now"
    );

    // ... and only now does the second archive appear.
    build_zip_fixture(&source, "second.zip", &[("nested/b.bin", b"b")]);

    handle_process_action(&mut state, ProcessAction::RunPipeline, &shared);
    let run = wait_for_run_to_finish(&shared);

    assert!(
        destination.join("first").exists() && destination.join("second").exists(),
        "both archives in the folder must be processed; run log: {:?}",
        run.log
    );
}

// ── cancellation ────────────────────────────────────────────────────────

/// The page's Cancel reaches the operation registry, and the registry
/// stops the batch.
///
/// The pre-facade runner could not do this at all: it called
/// `execute_pipeline` once for the whole batch inside one
/// `spawn_blocking`, with its own comment recording that "mid-execution
/// cancellation is not possible with the current blocking executor". The
/// only cancellation it had was a flag checked *before* the batch
/// started.
///
/// The batch is deliberately wide (60 inputs, each of which costs an
/// extraction, a staged directory copy and two database writes) while
/// the cancel is issued as soon as the run reports its first progress —
/// the same shape as the facade suite's own between-files cancellation
/// test, with the margin coming from batch width rather than from a
/// backend gate.
#[test]
fn cancelling_the_page_s_run_reaches_the_registry_and_stops_the_batch() {
    const INPUTS: usize = 60;

    let (temp, shared) = create_test_shared_state_with_facade();
    let mut state = seeded_page();

    let source = temp.path().join("inbox");
    std::fs::create_dir_all(&source).unwrap();
    for index in 0..INPUTS {
        build_zip_fixture(
            &source,
            &format!("RJ{:06}.zip", 100_000 + index),
            &[("nested/data.bin", b"payload")],
        );
    }
    let destination = temp.path().join("out");
    flatten_to_folder(
        &mut state,
        PipelineInputsDto::Folder {
            path: source.clone(),
        },
        &destination,
    );

    handle_process_action(&mut state, ProcessAction::RunPipeline, &shared);
    let operation_id = wait_for_operation_id(&shared);

    // Wait for the batch to actually be under way before cancelling, so
    // this cancels a running operation rather than racing its start.
    let deadline = Instant::now() + Duration::from_secs(30);
    while shared.signals().process_run.get().files_total == 0 {
        assert!(Instant::now() < deadline, "the run never reported progress");
        std::thread::sleep(Duration::from_millis(2));
    }

    process_runner::cancel_pipeline_run(&shared);
    let run = wait_for_run_to_finish(&shared);

    assert!(
        run.cancelled,
        "the run must report itself cancelled; log: {:?}",
        run.log
    );

    // The registry -- not the page -- is what actually holds the
    // cancelled state, which is the point of routing Cancel through it.
    let snapshot = shared
        .services
        .tokio_runtime
        .block_on(shared.facade.as_ref().unwrap().operation(operation_id))
        .expect("the cancelled operation must still be queryable");
    assert_eq!(
        snapshot.state,
        arclain_app::event::OperationState::Cancelled
    );

    let written = std::fs::read_dir(&destination)
        .map(|dir| dir.count())
        .unwrap_or(0);
    assert!(
        written < INPUTS,
        "cancelling must stop inputs that had not started; {written} of {INPUTS} were processed"
    );
}

/// Cancelling with nothing in flight is a no-op, not a panic or a stale
/// operation id being handed to the registry.
#[test]
fn cancelling_with_no_run_in_flight_is_a_no_op() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    process_runner::cancel_pipeline_run(&shared);
    assert!(shared.signals().process_run.get().operation_id.is_none());
}

// ── presets ─────────────────────────────────────────────────────────────

/// Save, apply and delete, all through the application.
///
/// The page holds no presets path and no `SavedPreset` list of its own
/// any more: `LoadPresets`/`SavePreset`/`DeletePreset` are the
/// application's three methods, and each write hands back the full
/// updated list, so the page never has to re-read to stay current.
#[test]
fn presets_save_apply_and_delete_through_the_application() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let mut state = seeded_page();

    let destination = temp.path().join("preset-out");
    flatten_to_folder(
        &mut state,
        PipelineInputsDto::Files {
            paths: vec![temp.path().join("RJ123456.zip")],
        },
        &destination,
    );

    handle_process_action(
        &mut state,
        ProcessAction::SavePreset {
            name: "Flatten to folder".to_string(),
        },
        &shared,
    );
    assert_eq!(
        state.active_preset_name.as_deref(),
        Some("Flatten to folder")
    );
    let saved = state
        .presets()
        .iter()
        .find(|preset| preset.name == "Flatten to folder")
        .expect("the saved preset must come back in the application's list")
        .clone();
    assert!(
        !saved.builtin,
        "a user preset is not one of the shipped ones"
    );
    assert_eq!(saved.output_artifact, OutputArtifactDto::Folder);

    // A fresh page loads it from the application, not from a path the
    // page resolved itself.
    let mut reopened = seeded_page();
    reopened.presets = None;
    handle_process_action(&mut reopened, ProcessAction::LoadPresets, &shared);
    let listed = reopened
        .presets()
        .iter()
        .find(|preset| preset.name == "Flatten to folder")
        .expect("a fresh page must list the preset the previous one saved")
        .clone();

    // Applying takes the preset's steps and output settings and leaves
    // the current input alone.
    let mut applying = seeded_page();
    applying.draft.inputs = PipelineInputsDto::Folder {
        path: temp.path().join("somewhere-else"),
    };
    applying.apply_preset(&listed);
    assert_eq!(applying.draft.steps, saved.steps);
    assert_eq!(applying.draft.destination, saved.destination);
    assert_eq!(applying.draft.output_artifact, OutputArtifactDto::Folder);
    assert_eq!(
        applying.draft.inputs,
        PipelineInputsDto::Folder {
            path: temp.path().join("somewhere-else")
        },
        "applying a preset must not disturb the chosen input"
    );
    assert!(
        applying.preview_dirty,
        "applying a preset needs a re-preview"
    );

    handle_process_action(
        &mut state,
        ProcessAction::DeletePreset {
            name: "Flatten to folder".to_string(),
        },
        &shared,
    );
    assert!(
        state.active_preset_name.is_none(),
        "deleting the selected preset clears the selection"
    );
    assert!(
        !state
            .presets()
            .iter()
            .any(|preset| preset.name == "Flatten to folder"),
        "the deleted preset must be gone from the application's list"
    );
}

/// Saving twice under one name edits it in place rather than leaving two
/// rows the dropdown renders identically. The pre-facade Save button
/// appended unconditionally, and its minute-resolution default name made
/// that reachable by clicking Save twice.
#[test]
fn re_saving_a_preset_replaces_it_rather_than_duplicating_it() {
    let (temp, shared) = create_test_shared_state_with_facade();
    let mut state = seeded_page();
    let destination = temp.path().join("preset-out");
    flatten_to_folder(
        &mut state,
        PipelineInputsDto::Files {
            paths: vec![temp.path().join("RJ123456.zip")],
        },
        &destination,
    );

    handle_process_action(
        &mut state,
        ProcessAction::SavePreset {
            name: "Repeat".to_string(),
        },
        &shared,
    );
    state.draft.steps.push(PipelineStepDto::Flatten {
        strip_common_prefix: true,
        max_depth: 0,
    });
    handle_process_action(
        &mut state,
        ProcessAction::SavePreset {
            name: "Repeat".to_string(),
        },
        &shared,
    );

    let matches: Vec<_> = state
        .presets()
        .iter()
        .filter(|preset| preset.name == "Repeat")
        .collect();
    assert_eq!(matches.len(), 1, "a re-save must not append a second row");
    assert_eq!(
        matches[0].steps.len(),
        2,
        "the stored preset must be the edited one"
    );
}

// ── interrupted-run banner ──────────────────────────────────────────────

/// The banner's count is the application's answer to
/// `interrupted_pipeline_runs`, not a database query the page runs.
///
/// A fresh profile has none, so the banner does not render. Its
/// permanence once one exists is a property of the surface, not of this
/// page: nothing ever clears an interrupted run, so a banner keyed on
/// "since 0" reappears on every launch until the user dismisses it
/// again -- recorded here rather than worked around.
#[test]
fn the_interrupted_banner_reads_the_application_and_a_fresh_profile_has_none() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let mut state = ProcessPageState::default();

    handle_process_action(&mut state, ProcessAction::LoadInterruptedCount, &shared);

    assert_eq!(state.interrupted_run_count, Some(0));
    assert_eq!(state.interrupted_run_label(), "0");
}

/// A saturated answer is reported as "N+", because
/// `interrupted_pipeline_runs` bounds the answer and not the query --
/// the page must not present a bounded count as an exact one.
#[test]
fn a_saturated_interrupted_count_is_labelled_as_a_lower_bound() {
    let mut state = ProcessPageState::default();

    state.interrupted_run_count =
        Some(arclain_ui::features::process::state::INTERRUPTED_RUN_QUERY_LIMIT as usize);
    assert_eq!(
        state.interrupted_run_label(),
        format!(
            "{}+",
            arclain_ui::features::process::state::INTERRUPTED_RUN_QUERY_LIMIT
        )
    );

    state.interrupted_run_count = Some(3);
    assert_eq!(state.interrupted_run_label(), "3");
}
