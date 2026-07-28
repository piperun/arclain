//! Integration tests for the batch processing operations --
//! `start_convert`/`start_organize`/`start_pipeline` -- driven through
//! `ArclainApp`'s public facade, the same way a real frontend would.
//!
//! Characterization this suite locks in (see each request type's own
//! doc comment in `crates/app/src/operations/{convert,organize,
//! pipeline}.rs` and `crates/app/src/runtime/processing_ops.rs` for the
//! full reasoning):
//!
//! - Structural validation (empty inputs, an unparseable `format`/
//!   `profile_id`, an unknown rule/preset id) is rejected *before* an
//!   operation is ever registered -- no phantom `OperationId` for a
//!   malformed request.
//! - A batch operation processes its inputs one at a time; cancelling
//!   mid-batch stops any input that has not yet started, but (matching
//!   the pre-facade UI's own documented limitation -- see
//!   `crates/ui/src/core/operations/process_runner.rs`'s "Mid-execution
//!   cancellation is not possible" comment) does not interrupt an input
//!   already in flight.
//! - A per-file failure (bad archive, output collision) is reported via
//!   the progress summary and counted as `failed`; it does not turn the
//!   *operation* itself `Failed` -- matching `execute_pipeline`'s own
//!   "keep going, tally the outcome" semantics the pre-facade UI already
//!   relied on.
//! - Output-transaction rollback (an existing, unrecognized destination
//!   is never touched by a losing/colliding run) holds through the
//!   facade exactly as it does in `arclain_core`'s own `StagedOutput`
//!   test suite -- these tests prove the facade's wiring doesn't bypass
//!   that guarantee, not the guarantee itself.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! for the same reason `archive_sessions.rs`/`bootstrap.rs` are: dropping
//! `ArclainApp` (which owns its own Tokio runtime) must not happen from
//! inside an async context.

mod support;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationEvent, OperationKind, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::operations::{ConvertRequest, OrganizeRequest, PipelineRequest};
use arclain_app::{ArclainApp, BootstrapConfig};

use arclain_core::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

/// Bootstraps an isolated `ArclainApp`, optionally overriding the
/// archive-extraction backend (test seam, same field `tests/
/// archive_sessions.rs` uses) and/or the pipeline-presets file path
/// (new seam this task adds -- production always resolves the real
/// OS-conventional presets path; tests need an isolated one so they
/// never read/write the developer's actual `pipeline_presets.json`).
fn bootstrap_app_ex(
    temp: &tempfile::TempDir,
    archive_backend_override: Option<std::sync::Arc<dyn ArchiveBackend>>,
    presets_path_override: Option<PathBuf>,
) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override,
        extract_runner_override: None,
        presets_path_override,
    })
    .expect("bootstrap must succeed")
}

fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    bootstrap_app_ex(temp, None, None)
}

/// Builds a ZIP fixture at `dir/name` containing `entries` (archive-
/// relative path -> content). Mirrors `archive_sessions.rs`'s own
/// helper of the same shape; duplicated rather than shared since
/// integration test files cannot import from one another.
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

/// Writes `count` non-zero-but-arbitrary bytes to a garbage (not a real
/// ZIP) file at `path` -- real zip parsing fails on this deterministically,
/// without depending on any fake backend.
fn write_garbage_file(path: &Path, byte: u8) {
    std::fs::write(path, vec![byte; 64]).expect("write garbage fixture file");
}

/// Seeds a minimal, deterministic organization rule directly through the
/// app's own composed `OrganizationService` (via `take_legacy_composition`
/// -- an `Arc` clone of the exact service `AppRuntime` itself holds, so
/// writes land in the same database `start_organize`'s rule lookup reads
/// from). The rule moves every top-level file into `Organized/<name>`
/// unconditionally (`pattern: "*"`, `target: ""`, `use_standard_layout:
/// false`) -- enough to prove the organize operation's wiring without
/// depending on metadata-driven rule matching.
fn seed_flat_organize_rule(app: &ArclainApp) -> i64 {
    let legacy = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed for a freshly bootstrapped app");
    let organization_service = legacy
        .core_services
        .organization_service
        .clone()
        .expect("organization_service must be composed for a freshly bootstrapped app");
    let rule = arclain_core::OrganizationRule {
        id: 0,
        name: "flat-move-all".to_string(),
        priority: 0,
        is_enabled: true,
        trigger: arclain_core::RuleTrigger::default(),
        actions: arclain_core::RuleActions {
            root_folder: Some("Organized".to_string()),
            output_name: None,
            move_files: vec![arclain_core::MoveAction {
                // `"**"` is the one pattern `RuleEngine::matches_glob`
                // treats as "match everything" -- a bare `"*"` is *not*
                // special-cased there (it only special-cases `"**"` and
                // `"*.ext"`), so it would fall through to an exact/
                // filename match and never match `payload.bin`/`data.bin`.
                pattern: "**".to_string(),
                target: String::new(),
            }],
            use_standard_layout: false,
        },
    };
    organization_service
        .save_domain_rule(&rule)
        .expect("seeding the test organization rule must succeed")
}

/// Saves one custom, hermetic pipeline preset (a single `Flatten` step,
/// `Folder` output -- no 7-Zip CLI or organization rule dependency) at
/// `path`, so `start_pipeline` tests never depend on the developer's
/// real presets file or a real archive-packing tool.
fn seed_flatten_only_preset(path: &Path, preset_name: &str) {
    use arclain_core::{OutputArtifact, Pipeline, PipelineOutput, PipelineStep, SavedPreset};
    let preset = SavedPreset {
        name: preset_name.to_string(),
        pipeline: Pipeline {
            input: None,
            steps: vec![PipelineStep::Flatten {
                strip_common_prefix: false,
                max_depth: 1,
            }],
            output: PipelineOutput::SameFolder,
            collision_policy: None,
            output_artifact: OutputArtifact::Folder,
        },
    };
    arclain_core::save_presets(path, &[preset]).expect("saving the test preset must succeed");
}

/// A fake `ArchiveBackend` whose `extract_all` always "succeeds" by
/// writing one known file (`payload.bin`) into `dest`, regardless of
/// what `path` actually points at on disk -- letting cancellation tests
/// avoid depending on real archive content. When constructed via
/// [`Self::gated`], `extract_all` for one specific input path blocks on
/// a channel until the test explicitly releases it, so a test can land
/// `cancel_operation` deterministically while that input is still being
/// processed, instead of racing real timing.
///
/// Every other `ArchiveBackend` method is `unimplemented!()`: the
/// processing operations under test never call them (only extraction is
/// exercised before the pure organize-planning/output-transaction code
/// takes over).
struct FakeExtractBackend {
    gate: Option<(PathBuf, Mutex<Option<mpsc::Receiver<()>>>)>,
}

impl FakeExtractBackend {
    /// Returns the backend plus the sender a test uses to release the
    /// gate once it has observed enough to know cancellation landed.
    fn gated(gated_path: PathBuf) -> (std::sync::Arc<dyn ArchiveBackend>, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel();
        let backend = Self {
            gate: Some((gated_path, Mutex::new(Some(rx)))),
        };
        (std::sync::Arc::new(backend), tx)
    }
}

impl ArchiveBackend for FakeExtractBackend {
    fn name(&self) -> &str {
        "fake-extract"
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
        Ok(ArchiveKind::Zip)
    }
    fn list(&self, _path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
        unimplemented!("not exercised by the processing operations under test")
    }
    fn extract_all(&self, path: &Path, dest: &Path, _password: Option<&str>) -> anyhow::Result<()> {
        if let Some((gated_path, receiver)) = &self.gate {
            if path == gated_path {
                if let Some(receiver) = receiver.lock().unwrap().take() {
                    let _ = receiver.recv();
                }
            }
        }
        std::fs::create_dir_all(dest)?;
        std::fs::write(dest.join("payload.bin"), b"fake extracted content")?;
        Ok(())
    }
    fn extract_files(
        &self,
        _p: &Path,
        _d: &Path,
        _f: &[String],
        _pw: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _p: &Path,
        _d: &Path,
        _dp: &str,
        _pw: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn recompress_7z(&self, _s: &Path, _d: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_files(&self, _a: &Path, _f: &[PathBuf]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn create_archive(&self, _d: &Path, _f: &[PathBuf], _fmt: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn read_text_file(&self, _a: &Path, _p: &str, _pw: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn delete_files(&self, _a: &Path, _f: &[String]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_or_update_file_from_str(&self, _a: &Path, _p: &str, _c: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn convert_to_7z(
        &self,
        _s: &arclain_core::Archive,
        _d: &Path,
        _t: &Path,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn crc32_of_entry(&self, _a: &Path, _p: &str, _pw: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
}

/// Drains the operation-event stream (filtering to `operation_id`,
/// ignoring events from any other operation) until a terminal state is
/// reached, collecting every `Progress` message seen along the way.
/// Bounded so a real bug (the operation getting stuck) fails the test
/// instead of hanging the suite.
async fn drain_until_terminal(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
) -> (Vec<String>, OperationState) {
    let mut messages = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), receiver.recv())
            .await
            .expect("operation event must arrive within 15s")
            .expect("operation event channel must not close");
        if event.operation_id != operation_id {
            continue;
        }
        if let OperationState::Progress {
            message: Some(message),
            ..
        } = &event.state
        {
            messages.push(message.clone());
        }
        if matches!(
            event.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return (messages, event.state);
        }
    }
}

/// Drains events until a `Progress` message containing `needle` is seen,
/// then returns -- without waiting for a terminal state. Used by the
/// cancellation test to know the worker has reached (and is now gated
/// inside) a specific input's extraction step.
async fn wait_for_message_containing(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
    needle: &str,
) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), receiver.recv())
            .await
            .expect("operation event must arrive within 15s")
            .expect("operation event channel must not close");
        if event.operation_id != operation_id {
            continue;
        }
        if let OperationState::Progress {
            message: Some(message),
            ..
        } = &event.state
        {
            if message.contains(needle) {
                return;
            }
        }
    }
}

/// Polls for `path` to exist, up to `timeout`. Used only where the
/// facade itself gives no further event-stream signal to await -- see
/// `start_organize_between_files_cancellation_stops_unstarted_inputs`'s
/// own comment for why that is the correct, non-racy thing to do there.
async fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while !path.exists() {
        if std::time::Instant::now() >= deadline {
            panic!("expected {path:?} to exist within {timeout:?}, it never appeared");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn assert_no_operation_was_registered(app: &ArclainApp, runtime: &tokio::runtime::Runtime) {
    let recent = runtime
        .block_on(app.recent_operations(10))
        .expect("recent_operations must succeed");
    assert!(
        recent.is_empty(),
        "a structurally invalid request must not register a phantom operation: {recent:?}"
    );
}

// ─── validation: rejected before any operation is registered ───────────

#[test]
fn start_convert_rejects_empty_inputs() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_convert(ConvertRequest {
            inputs: vec![],
            destination: temp.path().join("out"),
            format: "zip".to_string(),
            flatten: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("inputs"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_convert_rejects_unknown_format() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_convert(ConvertRequest {
            inputs: vec![temp.path().join("a.rar")],
            destination: temp.path().join("out"),
            format: "rar".to_string(),
            flatten: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("format"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_empty_inputs() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![],
            destination: temp.path().join("out"),
            profile_id: "1".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("inputs"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_non_numeric_profile_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: "not-a-number".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("profile_id"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_unknown_rule_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: "999999".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_empty_inputs() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: vec![],
            destination: temp.path().join("out"),
            preset_id: "RE Mod Cleanup".to_string(),
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("inputs"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_unknown_preset_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: vec![temp.path().join("a.rar")],
            destination: temp.path().join("out"),
            preset_id: "no-such-preset".to_string(),
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_no_operation_was_registered(&app, &runtime);
}

// ─── organize: happy path, dry run, cancellation ───────────────────────

#[test]
fn start_organize_completes_and_organizes_each_input_into_its_own_destination_folder() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let rule_id = seed_flat_organize_rule(&app);

    let alpha = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let beta = build_zip_fixture(temp.path(), "beta.zip", &[("data.bin", b"beta-content")]);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![alpha, beta],
                destination: destination.clone(),
                profile_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        let snapshot = app.operation(operation_id).await.unwrap();
        assert_eq!(snapshot.kind, OperationKind::Organize);

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(
            messages.iter().any(|m| m.contains("2 succeeded")),
            "expected a summary message reporting 2 successes, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(destination.join("alpha/Organized/data.bin")).unwrap(),
        b"alpha-content"
    );
    assert_eq!(
        std::fs::read(destination.join("beta/Organized/data.bin")).unwrap(),
        b"beta-content"
    );
}

#[test]
fn start_organize_dry_run_reports_a_preview_and_never_touches_the_filesystem() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let rule_id = seed_flat_organize_rule(&app);

    let alpha = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![alpha],
                destination: destination.clone(),
                profile_id: rule_id.to_string(),
                dry_run: true,
            })
            .await
            .expect("start_organize (dry_run) must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        let expected_operation = format!("Apply organization rule #{rule_id}");
        assert!(
            messages.iter().any(|m| m.contains(&expected_operation)),
            "expected a preview message naming the rule, got: {messages:?}"
        );
    });

    assert!(
        !destination.exists(),
        "dry_run must never create the destination folder"
    );
}

#[test]
fn start_organize_between_files_cancellation_stops_unstarted_inputs() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();

    let gated_input = temp.path().join("first.dat");
    std::fs::write(
        &gated_input,
        b"hashable placeholder content for first input",
    )
    .unwrap();
    let unstarted_input = temp.path().join("second.dat");
    std::fs::write(
        &unstarted_input,
        b"hashable placeholder content for second input",
    )
    .unwrap();

    let (backend, release) = FakeExtractBackend::gated(gated_input.clone());
    let app = bootstrap_app_ex(&temp, Some(backend), None);
    let rule_id = seed_flat_organize_rule(&app);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![gated_input.clone(), unstarted_input.clone()],
                destination: destination.clone(),
                profile_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        // Wait until the worker has dispatched the first input's
        // extraction (and is now blocked inside it) before cancelling --
        // this is what makes the assertion below deterministic instead
        // of racing real timing (see this test's own module doc comment
        // and `crate::runtime::processing_ops`'s characterization notes).
        wait_for_message_containing(&mut receiver, operation_id, "Processing first.dat").await;

        app.cancel_operation(operation_id)
            .await
            .expect("cancel_operation must succeed while the operation is still running");

        // Only now let the first input's (already in-flight, and per the
        // pre-facade UI's own documented limitation not interruptible
        // mid-file) extraction proceed.
        release.send(()).expect("release the gated extraction");

        let (_messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(terminal, OperationState::Cancelled);

        // `OperationRegistry::transition` (by design -- see its own
        // "terminal state ignores further transitions" test) silently
        // drops every further progress event for this operation once it
        // is `Cancelled`, so the worker's continued handling of the
        // already-in-flight first input becomes invisible on the event
        // stream from this point on -- but its filesystem effects still
        // land once the underlying `execute_pipeline` call finishes.
        // Poll briefly for that, rather than asserting immediately: the
        // facade gives no further signal for exactly when that trailing
        // work completes (see this task's report).
        wait_for_path(&destination.join("first"), Duration::from_secs(5)).await;
    });

    assert!(
        destination.join("first").exists(),
        "the input already in flight when cancel landed must still finish"
    );
    assert!(
        !destination.join("second").exists(),
        "an input that had not started yet must never be processed after cancellation"
    );
}

// ─── output-transaction rollback: pre-existing destinations survive ───

#[test]
fn start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);

    let input = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let destination = temp.path().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    let collision_path = destination.join("alpha.zip");
    std::fs::write(&collision_path, b"known-good-existing-archive").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_convert(ConvertRequest {
                inputs: vec![input],
                destination: destination.clone(),
                format: "zip".to_string(),
                flatten: false,
            })
            .await
            .expect("start_convert must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "a per-file collision must not turn the whole operation Failed"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the collision as a failed file, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(&collision_path).unwrap(),
        b"known-good-existing-archive",
        "a colliding run must never touch an existing, unrecognized destination file"
    );
}

#[test]
fn start_organize_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let rule_id = seed_flat_organize_rule(&app);

    let input = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let destination = temp.path().join("out");
    let collision_path = destination.join("alpha");
    std::fs::create_dir_all(&collision_path).unwrap();
    std::fs::write(collision_path.join("canary.txt"), b"do-not-touch").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input],
                destination: destination.clone(),
                profile_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "a per-file collision must not turn the whole operation Failed"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the collision as a failed file, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(collision_path.join("canary.txt")).unwrap(),
        b"do-not-touch",
        "a colliding run must never touch an existing, unrecognized destination folder"
    );
    assert!(
        !collision_path.join("Organized").exists(),
        "the colliding run's own output must never be staged into the pre-existing folder"
    );
}

#[test]
fn start_organize_extraction_failure_leaves_no_partial_output() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let rule_id = seed_flat_organize_rule(&app);

    let bogus = temp.path().join("bogus.zip");
    write_garbage_file(&bogus, 0xAA);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![bogus],
                destination: destination.clone(),
                profile_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "an extraction failure must not turn the whole operation Failed"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the extraction failure, got: {messages:?}"
        );
    });

    assert!(
        !destination.exists(),
        "a failed extraction must never create a partial destination folder"
    );
}

// ─── pipeline (saved preset) ────────────────────────────────────────────

#[test]
fn start_pipeline_runs_a_saved_preset_end_to_end() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let presets_path = temp.path().join("presets.json");
    seed_flatten_only_preset(&presets_path, "test-flatten-only");
    let app = bootstrap_app_ex(&temp, None, Some(presets_path));

    let input = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: destination.clone(),
                preset_id: "test-flatten-only".to_string(),
            })
            .await
            .expect("start_pipeline must be accepted");

        let snapshot = app.operation(operation_id).await.unwrap();
        assert_eq!(snapshot.kind, OperationKind::Pipeline);

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(
            messages.iter().any(|m| m.contains("1 succeeded")),
            "expected a summary message reporting 1 success, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(destination.join("alpha/data.bin")).unwrap(),
        b"alpha-content"
    );
}

#[test]
fn start_pipeline_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let presets_path = temp.path().join("presets.json");
    seed_flatten_only_preset(&presets_path, "test-flatten-only");
    let app = bootstrap_app_ex(&temp, None, Some(presets_path));

    let input = build_zip_fixture(temp.path(), "alpha.zip", &[("data.bin", b"alpha-content")]);
    let destination = temp.path().join("out");
    let collision_path = destination.join("alpha");
    std::fs::create_dir_all(&collision_path).unwrap();
    std::fs::write(collision_path.join("canary.txt"), b"do-not-touch").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: destination.clone(),
                preset_id: "test-flatten-only".to_string(),
            })
            .await
            .expect("start_pipeline must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the collision as a failed file, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(collision_path.join("canary.txt")).unwrap(),
        b"do-not-touch"
    );
}

// A real, real-7z-backed end-to-end proof of `ConvertRequest`'s Archive-
// output path was attempted here and deliberately removed. It surfaced a
// pre-existing bug in `arclain_core`, not this task's own code -- see
// this task's report for the full writeup. Left out rather than
// papered over with an assertion that tolerates the bug: this task
// does not touch `arclain_core`'s pipeline executor, and shipping a
// test that quietly accepts wrong behavior there would hide it instead
// of surfacing it.
