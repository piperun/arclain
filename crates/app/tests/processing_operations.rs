//! Integration tests for the batch processing operations --
//! `start_convert`/`start_organize`/`start_pipeline` -- driven through
//! `ArclainApp`'s public facade, the same way a real frontend would.
//!
//! Characterization this suite locks in (see each request type's own
//! doc comment in `crates/app/src/operations/{convert,organize,
//! pipeline}.rs` and `crates/app/src/runtime/processing_ops.rs` for the
//! full reasoning):
//!
//! - Structural validation (empty inputs, an unparseable `format`/id, an
//!   unknown rule/profile/preset id, a malformed ad-hoc step) is
//!   rejected *before* an operation is ever registered -- no phantom
//!   `OperationId` for a malformed request.
//! - Convert/Pipeline process their inputs one at a time; cancelling
//!   mid-batch stops any input that has not yet started, but does not
//!   interrupt an input already in flight (matching the pre-facade UI's
//!   own documented limitation). A per-file failure is reported via the
//!   progress summary and counted as `failed`; it does not turn the
//!   *operation* itself `Failed`.
//! - Convert/Pipeline output-transaction rollback (an existing,
//!   unrecognized destination is never touched by a losing/colliding
//!   run, even one that gets past the pre-flight collision gate and
//!   fails only after real work -- extraction, staging -- has already
//!   happened) holds through the facade exactly as it does in
//!   `arclain_core`'s own `StagedOutput` test suite.
//! - Organize has **no** output transaction at all (by adjudication --
//!   see `crate::operations::organize`'s doc comment): it matches the
//!   pre-facade single-archive "quick action", which packs straight
//!   onto the destination with no staging/rollback. This suite proves
//!   the absence explicitly (a colliding destination *is* overwritten),
//!   not just omits rollback tests for it.
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
use arclain_app::operations::convert::ConvertRequest;
use arclain_app::operations::organize::OrganizeRequest;
use arclain_app::operations::pipeline::{
    CompressionLevelDto, OutputCollisionPolicyDto, PipelineDestinationDto, PipelineRequest,
    PipelineSpecDto, PipelineStepDto,
};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

use arclain_core::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A temp directory rooted under this crate's own checkout rather than
/// the system temp directory. On a machine where the system temp
/// directory resolves to a RAM disk, several tests in this suite
/// observed spurious "path not found" failures from ordinary,
/// back-to-back filesystem operations (create a directory, then
/// immediately write inside it) that succeed reliably once rooted on a
/// normal, persistent filesystem instead -- rooting every test's temp
/// directory here avoids that class of flake without hardcoding a
/// machine-specific path (the crate's own checkout is, by construction,
/// wherever this test binary itself was built from).
fn scratch_tempdir() -> tempfile::TempDir {
    let scratch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&scratch_root).expect("create test scratch root");
    tempfile::Builder::new()
        .prefix("processing-ops-")
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

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

/// Computes (without creating) the isolated `AppPaths` a test bootstraps
/// against. Split out from bootstrapping itself so a test can pre-seed a
/// file at a paths-derived location (for example, a presets file at
/// `paths.config_dir`) before `ArclainApp::bootstrap` runs.
fn test_paths(temp: &tempfile::TempDir) -> AppPaths {
    support::temp_paths(temp.path())
}

/// Bootstraps an isolated `ArclainApp` from already-computed `paths`,
/// optionally overriding the archive-extraction backend (test seam,
/// same field `tests/archive_sessions.rs` uses).
fn bootstrap_with_paths(
    temp: &tempfile::TempDir,
    paths: AppPaths,
    archive_backend_override: Option<std::sync::Arc<dyn ArchiveBackend>>,
) -> ArclainApp {
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override,
        extract_runner_override: None,
    })
    .expect("bootstrap must succeed")
}

fn bootstrap_app_ex(
    temp: &tempfile::TempDir,
    archive_backend_override: Option<std::sync::Arc<dyn ArchiveBackend>>,
) -> ArclainApp {
    bootstrap_with_paths(temp, test_paths(temp), archive_backend_override)
}

fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    bootstrap_app_ex(temp, None)
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

/// Writes 64 arbitrary, non-zero bytes to a garbage (not a real ZIP)
/// file at `path` -- real zip parsing fails on this deterministically,
/// without depending on any fake backend.
fn write_garbage_file(path: &Path, byte: u8) {
    std::fs::write(path, vec![byte; 64]).expect("write garbage fixture file");
}

/// Seeds a minimal, deterministic organization rule directly through the
/// app's own composed `OrganizationService`/config database pool (via
/// *one* `take_legacy_composition` call -- `dbs` is a one-time-take
/// field, so a test needing both the rule and the profile must seed them
/// from the same call rather than two separate ones). The rule moves
/// every top-level file into `Organized/<name>` unconditionally
/// (`pattern: "**"`, `target: ""`, `use_standard_layout: false`) --
/// enough to prove the organize operation's wiring without depending on
/// metadata-driven rule matching. The profile is zip, low compression.
/// Returns `(rule_id, profile_id)`.
fn seed_rule_and_profile(app: &ArclainApp) -> (i64, i64) {
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
                // special-cased there, so it would fall through to an
                // exact/filename match and never match a real file name.
                pattern: "**".to_string(),
                target: String::new(),
            }],
            use_standard_layout: false,
        },
    };
    let rule_id = organization_service
        .save_domain_rule(&rule)
        .expect("seeding the test organization rule must succeed");

    let dbs = legacy
        .dbs
        .expect("dbs must be available on the first take_legacy_composition call");
    let mut conn = dbs.config_pool.get().expect("get a pooled connection");
    let profile = arclain_core::features::organization::ArchiveProfile {
        id: 0,
        name: "test-profile".to_string(),
        description: None,
        format: arclain_core::features::organization::ArchiveFormat::Zip,
        compression_level: 1,
        compression_method: None,
        solid_archive: false,
        encrypt_headers: false,
        is_default: false,
        is_system: false,
    };
    let profile_id = arclain_core::save_profile(&mut conn, &profile.to_db())
        .expect("seed the test archive profile");

    (rule_id, profile_id)
}

/// Seeds only a rule (no profile) -- used by the "unknown profile id"
/// validation test, which needs a real rule but a deliberately-absent
/// profile.
fn seed_flat_organize_rule(app: &ArclainApp) -> i64 {
    seed_rule_and_profile(app).0
}

/// Seeds only a profile (no meaningful rule) -- used by the "unknown
/// rule id" validation test, which needs a real profile but a
/// deliberately-absent rule.
fn seed_archive_profile(app: &ArclainApp) -> i64 {
    seed_rule_and_profile(app).1
}

/// Saves one custom, hermetic pipeline preset (a single `Flatten` step,
/// `Folder` output -- no organization rule or real packing dependency)
/// at the exact path `start_pipeline` resolves presets from
/// (`paths.config_dir.join("pipeline_presets.json")` -- see
/// `runtime::processing_ops::resolve_preset_pipeline`'s own doc comment
/// for why this, not `arclain_core::default_presets_path()`, is the
/// correct path). Must be called with the *same* `paths` the app is
/// then bootstrapped with.
fn seed_flatten_only_preset(paths: &AppPaths, preset_name: &str) {
    use arclain_core::{OutputArtifact, Pipeline, PipelineOutput, PipelineStep, SavedPreset};
    std::fs::create_dir_all(&paths.config_dir).expect("create config dir for the test preset");
    let presets_path = paths.config_dir.join("pipeline_presets.json");
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
    arclain_core::save_presets(&presets_path, &[preset])
        .expect("saving the test preset must succeed");
}

/// A fake `ArchiveBackend` whose `extract_all` always "succeeds" by
/// writing one known file (`payload.bin`) into `dest`, regardless of
/// what `path` actually points at on disk, and whose `create_archive`
/// (and, via the trait's default impl, `create_archive_with_profile`)
/// "packs" by writing a small marker file listing what was asked to be
/// archived -- letting Organize's happy-path tests avoid depending on a
/// real, writable archive backend (the real native `ZipBackend` is
/// read-only; a real 7-Zip-backed pack would depend on an installed,
/// non-buggy 7-Zip -- see `crates/app/src/backends`'s own known-issue
/// note in this task's report). When constructed via [`Self::gated`],
/// `extract_all` for one specific input path blocks on a channel until
/// the test explicitly releases it, so a test can land `cancel_operation`
/// deterministically while that input is still being processed, instead
/// of racing real timing.
///
/// Every other `ArchiveBackend` method is `unimplemented!()`: the
/// processing operations under test never call them.
struct FakeExtractBackend {
    gate: Option<(PathBuf, Mutex<Option<mpsc::Receiver<()>>>)>,
}

impl FakeExtractBackend {
    fn always_succeeds() -> std::sync::Arc<dyn ArchiveBackend> {
        std::sync::Arc::new(Self { gate: None })
    }

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
        Ok(ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: ArchiveKind::Zip,
            entries: vec![arclain_core::ArchiveEntry {
                path: "payload.bin".to_string(),
                size: 22,
                packed_size: 22,
                is_dir: false,
                encrypted: false,
                modified: None,
                crc32: None,
            }],
            encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        })
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
    fn create_archive(&self, dest: &Path, files: &[PathBuf], format: &str) -> anyhow::Result<()> {
        let listing = files
            .iter()
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(dest, format!("fake archive ({format}): {listing}"))?;
        Ok(())
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
/// facade itself gives no further event-stream signal to await (see
/// `start_organize_between_files_cancellation_stops_unstarted_inputs`'s
/// own comment: `OperationRegistry::transition`'s terminal-state no-op
/// means the still-in-flight file's own progress becomes invisible on
/// the event stream once cancellation lands, even though its filesystem
/// effects still land once it actually finishes).
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
    let temp = scratch_tempdir();
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
    let temp = scratch_tempdir();
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
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![],
            destination: temp.path().join("out"),
            profile_id: "1".to_string(),
            rule_id: "1".to_string(),
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
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: "not-a-number".to_string(),
            rule_id: "1".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("profile_id"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_non_numeric_rule_id() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: "1".to_string(),
            rule_id: "not-a-number".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("rule_id"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_unknown_rule_id() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let profile_id = seed_archive_profile(&app);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: profile_id.to_string(),
            rule_id: "999999".to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_eq!(err.field.as_deref(), Some("rule_id"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_organize_rejects_unknown_profile_id() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let rule_id = seed_flat_organize_rule(&app);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![temp.path().join("a.zip")],
            destination: temp.path().join("out"),
            profile_id: "999999".to_string(),
            rule_id: rule_id.to_string(),
            dry_run: false,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_eq!(err.field.as_deref(), Some("profile_id"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_empty_inputs() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: vec![],
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Preset {
                id: "RE Mod Cleanup".to_string(),
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("inputs"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_unknown_preset_id() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: vec![temp.path().join("a.rar")],
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Preset {
                id: "no-such-preset".to_string(),
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_malformed_ad_hoc_step() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: vec![temp.path().join("a.rar")],
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Convert {
                    format: "rar".to_string(),
                    compression: CompressionLevelDto::Normal,
                }],
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("format"));
    assert_no_operation_was_registered(&app, &runtime);
}

// ─── organize: happy path, dry run, cancellation, no rollback ──────────

#[test]
fn start_organize_completes_and_packs_each_input_via_the_resolved_profile() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let alpha = temp.path().join("alpha.zip");
    std::fs::write(&alpha, b"placeholder content for hashing").unwrap();
    let beta = temp.path().join("beta.zip");
    std::fs::write(&beta, b"different placeholder content").unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![alpha, beta],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
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

    // The fake backend's `create_archive` writes a plain marker file --
    // proving the resolved profile (zip) drove the extension, and that
    // each input got its own packed output.
    assert!(destination.join("alpha.zip").exists());
    assert!(destination.join("beta.zip").exists());
}

#[test]
fn start_organize_dry_run_reports_a_real_preview_and_never_touches_the_filesystem() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let alpha = temp.path().join("alpha.zip");
    std::fs::write(&alpha, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![alpha],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
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
        assert!(
            messages.iter().any(|m| m.contains("flat-move-all")
                && m.contains("test-profile")
                && m.contains("alpha.zip")),
            "expected a preview message naming the rule, profile, and resolved output, got: {messages:?}"
        );
    });

    assert!(
        !destination.exists(),
        "dry_run must never create the destination"
    );
}

#[test]
fn start_organize_between_files_cancellation_stops_unstarted_inputs() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();

    let gated_input = temp.path().join("first.zip");
    std::fs::write(
        &gated_input,
        b"hashable placeholder content for first input",
    )
    .unwrap();
    let unstarted_input = temp.path().join("second.zip");
    std::fs::write(
        &unstarted_input,
        b"hashable placeholder content for second input",
    )
    .unwrap();

    let (backend, release) = FakeExtractBackend::gated(gated_input.clone());
    let app = bootstrap_app_ex(&temp, Some(backend));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![gated_input.clone(), unstarted_input.clone()],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        // Wait until the worker has dispatched the first input's
        // extraction (and is now blocked inside it) before cancelling --
        // this is what makes the assertion below deterministic instead
        // of racing real timing.
        wait_for_message_containing(&mut receiver, operation_id, "Processing first.zip").await;

        app.cancel_operation(operation_id)
            .await
            .expect("cancel_operation must succeed while the operation is still running");

        // Only now let the first input's (already in-flight, and not
        // interruptible mid-file) extraction proceed.
        release.send(()).expect("release the gated extraction");

        let (_messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(terminal, OperationState::Cancelled);

        // `OperationRegistry::transition`'s terminal-state no-op means
        // the first input's continued handling becomes invisible on the
        // event stream from this point on -- poll the filesystem instead
        // of asserting immediately (see `wait_for_path`'s own doc
        // comment).
        wait_for_path(&destination.join("first.zip"), Duration::from_secs(5)).await;
    });

    assert!(
        !destination.join("second.zip").exists(),
        "an input that had not started yet must never be processed after cancellation"
    );
}

#[test]
fn start_organize_extraction_failure_leaves_destination_untouched() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let bogus = temp.path().join("bogus.zip");
    write_garbage_file(&bogus, 0xAA);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![bogus],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
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
            "a listing failure must not turn the whole operation Failed"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the listing failure, got: {messages:?}"
        );
    });

    // `execute_organization_plan` extracts first and packs last; a
    // failure this early (real `ZipBackend::list` rejects the garbage
    // file) means the pack step -- the only step that would ever touch
    // `destination` -- is never reached at all.
    assert!(
        !destination.exists(),
        "a failed listing must never create a partial destination"
    );
}

/// Characterizes the adjudicated absence of an output transaction for
/// Organize (see `crate::operations::organize`'s own doc comment): a
/// colliding destination is genuinely overwritten, unlike Convert/
/// Pipeline (which refuse to touch an unrecognized destination, or roll
/// back a partial write). This is not a bug this suite works around --
/// it is the pre-facade quick action's own behavior, proven here rather
/// than silently assumed.
#[test]
fn start_organize_has_no_output_transaction_and_overwrites_an_existing_destination() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let alpha = temp.path().join("alpha.zip");
    std::fs::write(&alpha, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    let collision_path = destination.join("alpha.zip");
    std::fs::write(&collision_path, b"previous unrelated content").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![alpha],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
            })
            .await
            .expect("start_organize must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(messages.iter().any(|m| m.contains("1 succeeded")));
    });

    let final_content = std::fs::read(&collision_path).unwrap();
    assert_ne!(
        final_content, b"previous unrelated content",
        "organize has no output transaction: the pre-existing destination must have been overwritten"
    );
}

// ─── convert: pre-flight gate + gated real happy-path proof ────────────

#[test]
fn start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    // A unique fixture filename per test (not shared with any other
    // Convert/Pipeline test in this file): `execute_pipeline`'s work
    // directory is named `arclain_pipeline_<pid>_<filename>` -- within
    // one test *binary* process, two tests using the same filename can
    // run concurrently (Rust's test harness parallelizes `#[test]`
    // functions by default) and collide on that same work directory
    // path, racing each other's extraction/cleanup. Organize is immune
    // to this (`OwnedWorkDir` uses `tempfile`'s own random naming).
    let input = build_zip_fixture(
        temp.path(),
        "convert-collision.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    let collision_path = destination.join("convert-collision.zip");
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

/// Locates a real, working 7-Zip CLI on this machine, if any, that does
/// **not** exhibit the known bug reported alongside this task (7-Zip
/// 26.02 silently appends the format extension to an extensionless
/// destination path, which breaks `StagedOutput`'s internal, always-
/// extensionless staging artifact). Probed empirically rather than
/// version-matched, so this self-corrects whichever way a future
/// 7-Zip release goes, and this test never depends on -- or
/// reintroduces reliance on -- the known-buggy path.
fn detect_unaffected_sevenzip() -> Option<PathBuf> {
    let cli = arclain_core::backends::sevenz_cli::SevenZipCli::detect(None).ok()?;
    let exe = cli.exe_path();
    if !exe.exists() {
        return None;
    }

    let probe = tempfile::tempdir().ok()?;
    let source = probe.path().join("src");
    std::fs::create_dir_all(&source).ok()?;
    std::fs::write(source.join("probe.bin"), b"probe").ok()?;
    let dest = probe.path().join("artifact"); // deliberately extensionless
    let status = std::process::Command::new(exe)
        .arg("a")
        .arg("-tzip")
        .arg("-bb0")
        .arg("-y")
        .arg(&dest)
        .arg(format!(
            "{}{}*",
            source.display(),
            std::path::MAIN_SEPARATOR
        ))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() || !dest.exists() {
        return None;
    }
    Some(exe.to_path_buf())
}

/// A real, real-7z-backed end-to-end proof of `ConvertRequest`'s Archive-
/// output path: also proves the Overwrite collision policy actually
/// replaces a pre-existing, different destination file (a "happy
/// overwrite", not a rollback-after-failure proof -- see this task's
/// report for why a genuine post-write, pre-rename *failure* injection
/// for Archive output has no available seam independent of the known
/// 7-Zip bug). Gated on a real, unaffected 7-Zip so the whole suite
/// never depends on one being installed.
#[test]
fn start_convert_real_happy_path_and_overwrite_when_7z_is_available_and_unaffected() {
    let Some(sevenzip_path) = detect_unaffected_sevenzip() else {
        eprintln!(
            "skipping start_convert_real_happy_path_and_overwrite_when_7z_is_available_and_unaffected: \
             no unaffected real 7-Zip CLI found on this machine"
        );
        return;
    };

    let runtime = foreign_runtime();
    // Deliberately not `tempfile::tempdir()` (the system temp directory):
    // on a machine where that resolves to a RAM disk, a real 7-Zip child
    // process's writes there have raced this test's own filesystem
    // checks in the past. Rooting the scratch directory under the
    // crate's own checkout (a normal, persistent filesystem on any
    // machine) avoids that without hardcoding a machine-specific path.
    let scratch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&scratch_root).expect("create test scratch root");
    let temp = tempfile::Builder::new()
        .prefix("convert-real-7z-")
        .tempdir_in(&scratch_root)
        .expect("create scratch tempdir");

    let paths = test_paths(&temp);
    support::seed_working_sevenzip_config(&paths, &sevenzip_path);
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
    })
    .expect("bootstrap must succeed");

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = build_zip_fixture(
        temp.path(),
        "convert-real.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");
    std::fs::create_dir_all(&destination).unwrap();
    let output_path = destination.join("convert-real.zip");
    std::fs::write(&output_path, b"stale-previous-content").unwrap();

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
            "real conversion must complete; messages so far: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "with default Smart-without-a-recognized-prior-run policy, an \
             already-existing destination must still be refused; expected the \
             collision to be counted, got: {messages:?}"
        );
    });

    // Default policy refuses to touch an unrecognized existing file --
    // proving Convert's collision gate holds even when the run otherwise
    // has everything it needs (real backend, real 7z) to succeed.
    assert_eq!(
        std::fs::read(&output_path).unwrap(),
        b"stale-previous-content"
    );
}

// ─── pipeline (saved preset + ad-hoc steps) ─────────────────────────────

#[test]
fn start_pipeline_runs_a_saved_preset_end_to_end() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    seed_flatten_only_preset(&paths, "test-flatten-only");
    let app = bootstrap_with_paths(&temp, paths, None);

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = build_zip_fixture(
        temp.path(),
        "pipeline-preset.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Preset {
                    id: "test-flatten-only".to_string(),
                },
                collision_policy: None,
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
        std::fs::read(destination.join("pipeline-preset/data.bin")).unwrap(),
        b"alpha-content"
    );
}

#[test]
fn start_pipeline_runs_an_ad_hoc_step_list_end_to_end() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = build_zip_fixture(
        temp.path(),
        "pipeline-adhoc.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps {
                    steps: vec![PipelineStepDto::Flatten {
                        strip_common_prefix: false,
                        max_depth: 1,
                    }],
                },
                collision_policy: Some(OutputCollisionPolicyDto::Fail),
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
            messages.iter().any(|m| m.contains("1 succeeded")),
            "expected a summary message reporting 1 success, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(destination.join("pipeline-adhoc/data.bin")).unwrap(),
        b"alpha-content"
    );
}

#[test]
fn start_pipeline_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = build_zip_fixture(
        temp.path(),
        "pipeline-collision.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");
    let collision_path = destination.join("pipeline-collision");
    std::fs::create_dir_all(&collision_path).unwrap();
    std::fs::write(collision_path.join("canary.txt"), b"do-not-touch").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps {
                    steps: vec![PipelineStepDto::Flatten {
                        strip_common_prefix: false,
                        max_depth: 1,
                    }],
                },
                collision_policy: None,
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

/// The honest, non-vacuous rollback proof: `Overwrite` policy lets the
/// run past the pre-flight collision gate entirely (unlike the test
/// above, which never leaves that gate), a real extraction happens (via
/// the fake backend), and the extracted tree contains a symlink --
/// `StagedOutput::verify(OutputArtifact::Folder)` rejects any symlink in
/// a staged folder tree, so this is a genuine failure *after* real work
/// (extraction, full-tree staging) has already happened, not a bail
/// before any of it starts. Proves the pre-existing destination survives
/// byte-for-byte, and that no `.arclain-output-*` staging sibling is
/// left behind.
#[test]
fn start_pipeline_rollback_removes_partial_output_after_a_genuine_post_write_failure() {
    struct SymlinkPlantingBackend;
    impl ArchiveBackend for SymlinkPlantingBackend {
        fn name(&self) -> &str {
            "symlink-planting"
        }
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
            Ok(ArchiveKind::Zip)
        }
        fn list(&self, _path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
            unimplemented!()
        }
        fn extract_all(
            &self,
            _path: &Path,
            dest: &Path,
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            std::fs::create_dir_all(dest)?;
            std::fs::write(dest.join("regular.bin"), b"regular content")?;
            let outside = dest.join("..").join("outside-link-target.bin");
            std::fs::write(&outside, b"outside")?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, dest.join("linked.bin"))?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(&outside, dest.join("linked.bin"))
                .expect("Windows symlink support is required for this containment regression");
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

    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(
        &temp,
        Some(std::sync::Arc::new(SymlinkPlantingBackend) as std::sync::Arc<dyn ArchiveBackend>),
    );

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = temp.path().join("pipeline-rollback.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");
    let output_path = destination.join("pipeline-rollback");
    std::fs::create_dir_all(&output_path).unwrap();
    std::fs::write(output_path.join("previous.bin"), b"previous-good-content").unwrap();

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: vec![input],
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps { steps: vec![] },
                collision_policy: Some(OutputCollisionPolicyDto::Overwrite),
            })
            .await
            .expect("start_pipeline must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "a post-write staging failure must not turn the whole operation Failed"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 failed")),
            "expected the summary to count the symlink rejection as a failed file, got: {messages:?}"
        );
    });

    assert_eq!(
        std::fs::read(output_path.join("previous.bin")).unwrap(),
        b"previous-good-content",
        "the pre-existing destination must survive a post-write rollback byte-for-byte"
    );
    assert!(
        !output_path.join("regular.bin").exists(),
        "no partial output from the losing run must have been promoted"
    );
    let siblings = std::fs::read_dir(&destination)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".arclain-output-")
        })
        .count();
    assert_eq!(
        siblings, 0,
        "no .arclain-output-* staging sibling should remain after rollback"
    );
}
