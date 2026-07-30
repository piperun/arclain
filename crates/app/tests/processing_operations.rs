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

use arclain_app::challenge::{Challenge, ChallengeResponse, SecretInput};
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationEvent, OperationKind, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::operations::convert::ConvertRequest;
use arclain_app::operations::organize::OrganizeRequest;
use arclain_app::operations::pipeline::{
    CompressionLevelDto, OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto,
    PipelineInputsDto, PipelineRequest, PipelineSpecDto, PipelineStepDto,
};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

use arclain_core::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};
use gameta_core::{MetadataSource, ProductMetadata};

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
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
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

/// Saves one custom, hermetic pipeline preset with **zero** steps and a
/// `Folder` output -- used only by the rollback test below, which needs
/// `start_pipeline` to run through `execute_pipeline`'s real staging/
/// verify path (extract -> stage -> `StagedOutput::verify` -> commit)
/// with nothing in between that could touch the fake backend's planted
/// symlink in an unintended way (a real pipeline step, e.g. `Flatten`,
/// might traverse or otherwise interact with a symlinked entry before
/// `StagedOutput::verify` ever gets a chance to reject it, which would
/// change the test's actual failure point without saying so).
/// `PipelineRequest::validate` rejects an empty ad-hoc
/// `PipelineSpecDto::Steps` list (see its own doc comment), so a
/// zero-step run can only be expressed as a saved preset here -- presets
/// are loaded as an already-built `arclain_core::Pipeline`, not
/// re-validated through `PipelineStepDto`.
fn seed_zero_step_preset(paths: &AppPaths, preset_name: &str) {
    use arclain_core::{OutputArtifact, Pipeline, PipelineOutput, SavedPreset};
    std::fs::create_dir_all(&paths.config_dir).expect("create config dir for the test preset");
    let presets_path = paths.config_dir.join("pipeline_presets.json");
    let preset = SavedPreset {
        name: preset_name.to_string(),
        pipeline: Pipeline {
            input: None,
            steps: vec![],
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
    /// The same mechanism one step earlier: blocks `list` rather than
    /// `extract_all`, so a test can act while the worker has started but
    /// has not yet built any plan. Armed separately, because opening the
    /// archive session lists it too and that call must not be caught.
    listing_gate: Option<ListingGate>,
}

/// A `list` gate: `armed` is flipped by the test once the setup that
/// also lists the archive is done, and `fired` records that a call
/// actually blocked here — without it a test whose gated path stops
/// matching would still pass, having silently exercised no window at
/// all.
struct ListingGate {
    path: PathBuf,
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    fired: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: Mutex<Option<mpsc::Receiver<()>>>,
}

impl FakeExtractBackend {
    fn always_succeeds() -> std::sync::Arc<dyn ArchiveBackend> {
        std::sync::Arc::new(Self {
            gate: None,
            listing_gate: None,
        })
    }

    /// Returns the backend plus the sender a test uses to release the
    /// gate once it has observed enough to know cancellation landed.
    fn gated(gated_path: PathBuf) -> (std::sync::Arc<dyn ArchiveBackend>, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel();
        let backend = Self {
            gate: Some((gated_path, Mutex::new(Some(rx)))),
            listing_gate: None,
        };
        (std::sync::Arc::new(backend), tx)
    }

    /// Returns the backend, the flag that arms its listing gate, the
    /// flag that records the gate firing, and the sender that releases
    /// it once armed.
    fn listing_gated(
        gated_path: PathBuf,
    ) -> (
        std::sync::Arc<dyn ArchiveBackend>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        mpsc::Sender<()>,
    ) {
        let (tx, rx) = mpsc::channel();
        let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = Self {
            gate: None,
            listing_gate: Some(ListingGate {
                path: gated_path,
                armed: armed.clone(),
                fired: fired.clone(),
                receiver: Mutex::new(Some(rx)),
            }),
        };
        (std::sync::Arc::new(backend), armed, fired, tx)
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
    fn list(&self, path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
        if let Some(gate) = &self.listing_gate {
            if path == gate.path && gate.armed.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(receiver) = gate.receiver.lock().unwrap().take() {
                    gate.fired.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = receiver.recv();
                }
            }
        }
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

/// A fake `ArchiveBackend` for Organize's password/challenge tests:
/// `list`/`extract_all` both require `correct_password` (matching
/// `archive_sessions.rs`'s own `FakeEncryptedBackend` for the same kind
/// of test there), combined with a working `extract_all`/`create_archive`
/// (matching this file's own `FakeExtractBackend`) so a full organize can
/// actually reach `Completed` -- proving the resolved password gets
/// threaded all the way through, not just that listing alone succeeds.
struct FakeEncryptedOrganizeBackend {
    correct_password: String,
}

fn fake_organize_info() -> ArchiveInfo {
    ArchiveInfo {
        archive_path: PathBuf::new(),
        archive_kind: ArchiveKind::Zip,
        entries: vec![arclain_core::ArchiveEntry {
            path: "payload.bin".to_string(),
            size: 22,
            packed_size: 22,
            is_dir: false,
            encrypted: true,
            modified: None,
            crc32: None,
        }],
        encrypted: true,
        headers_encrypted: false,
        encryption_method: Some("fake".to_string()),
    }
}

impl ArchiveBackend for FakeEncryptedOrganizeBackend {
    fn name(&self) -> &str {
        "fake-encrypted-organize"
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
        Ok(ArchiveKind::Zip)
    }
    fn list(&self, _path: &Path, password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
        match password {
            Some(candidate) if candidate == self.correct_password => Ok(fake_organize_info()),
            _ => Err(anyhow::anyhow!("Wrong password for archive")),
        }
    }
    fn extract_all(&self, _path: &Path, dest: &Path, password: Option<&str>) -> anyhow::Result<()> {
        match password {
            Some(candidate) if candidate == self.correct_password => {
                std::fs::create_dir_all(dest)?;
                std::fs::write(dest.join("payload.bin"), b"fake extracted content")?;
                Ok(())
            }
            _ => Err(anyhow::anyhow!("Wrong password for archive")),
        }
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

/// Drains events until a `Challenge` state is seen, then returns it --
/// without waiting for a terminal state. Used by the organize password/
/// challenge tests below, mirroring `archive_sessions.rs`'s own inline
/// loop for the same purpose.
async fn wait_for_challenge(
    receiver: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
) -> Challenge {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), receiver.recv())
            .await
            .expect("operation event must arrive within 15s")
            .expect("operation event channel must not close");
        if event.operation_id != operation_id {
            continue;
        }
        if let OperationState::Challenge { challenge } = event.state {
            return challenge;
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
            archive_session_id: None,
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
            archive_session_id: None,
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
            archive_session_id: None,
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
            archive_session_id: None,
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
            archive_session_id: None,
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
            inputs: PipelineInputsDto::Files { paths: vec![] },
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
            inputs: PipelineInputsDto::Files {
                paths: vec![temp.path().join("a.rar")],
            },
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Preset {
                id: "no-such-preset".to_string(),
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    // The field a caller/bridge should highlight is `pipeline` (where
    // the preset id actually lives, `PipelineSpecDto::Preset { id }`) --
    // not a nonexistent `preset_id` field on `PipelineRequest` itself.
    assert_eq!(err.field.as_deref(), Some("pipeline"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_malformed_ad_hoc_step() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: PipelineInputsDto::Files {
                paths: vec![temp.path().join("a.rar")],
            },
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Steps {
                steps: vec![PipelineStepDto::Convert {
                    format: "rar".to_string(),
                    compression: CompressionLevelDto::Normal,
                }],
                output_artifact: OutputArtifactDto::Archive,
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("format"));
    assert_no_operation_was_registered(&app, &runtime);
}

#[test]
fn start_pipeline_rejects_an_empty_ad_hoc_step_list() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);

    let err = runtime
        .block_on(app.start_pipeline(PipelineRequest {
            inputs: PipelineInputsDto::Files {
                paths: vec![temp.path().join("a.rar")],
            },
            destination: PipelineDestinationDto::SameFolder,
            pipeline: PipelineSpecDto::Steps {
                steps: vec![],
                output_artifact: OutputArtifactDto::default(),
            },
            collision_policy: None,
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(err.field.as_deref(), Some("steps"));
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
                archive_session_id: None,
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
                archive_session_id: None,
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
                archive_session_id: None,
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
        // 15s, matching every other wait budget in this suite
        // (`drain_until_terminal`, `wait_for_message_containing`,
        // `wait_for_challenge`) -- 5s here was tighter than all of those
        // for no reason, and running the full `arclain_app` suite
        // (competing with Task 6's own additional test binaries for the
        // same Tokio blocking-pool threads) surfaced it as a genuine,
        // if rare, flake: the in-flight write occasionally did not land
        // before this test's own comparatively tight deadline.
        wait_for_path(&destination.join("first.zip"), Duration::from_secs(15)).await;
    });

    assert!(
        !destination.join("second.zip").exists(),
        "an input that had not started yet must never be processed after cancellation"
    );
}

#[test]
fn start_organize_extraction_failure_leaves_destination_untouched() {
    // A deterministic fake backend whose `list` always fails with an
    // unambiguous, non-password-shaped error -- not the real
    // `BackendSelector` chain (`ZipBackend` -> dummy 7-Zip CLI fallback)
    // this test used before password handling existed. That chain's
    // fallback step, when the dummy (non-functional) 7-Zip binary this
    // suite seeds is invoked against a garbage file, produces a process-
    // exit-style error that `archive_ops::is_password_error` classifies
    // as password-shaped (a real, pre-existing ambiguity in that
    // classifier's exit-code patterns, shared with the open flow, not
    // introduced here) -- which made this test hang waiting for a
    // `Challenge::Password` response nobody sends, once Organize started
    // actually consulting that classifier. This backend sidesteps that
    // real-subprocess ambiguity entirely so the test asserts what it
    // always meant to: a genuine, unambiguous listing failure never
    // touches the destination and never turns the operation `Failed`.
    struct AlwaysCorruptBackend;
    impl ArchiveBackend for AlwaysCorruptBackend {
        fn name(&self) -> &str {
            "always-corrupt"
        }
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
            Ok(ArchiveKind::Zip)
        }
        fn list(&self, _path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
            // Deliberately not password-shaped -- matches one of the two
            // example strings `archive_ops::
            // is_password_error_rejects_unrelated_backend_errors` tests.
            Err(anyhow::anyhow!(
                "archive is corrupt: unexpected end of central directory"
            ))
        }
        fn extract_all(&self, _p: &Path, _d: &Path, _pw: Option<&str>) -> anyhow::Result<()> {
            unimplemented!()
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
        Some(std::sync::Arc::new(AlwaysCorruptBackend) as std::sync::Arc<dyn ArchiveBackend>),
    );
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
                archive_session_id: None,
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
    // failure this early (the fake backend's `list` always errors) means
    // the pack step -- the only step that would ever touch `destination`
    // -- is never reached at all.
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
                archive_session_id: None,
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

// ─── organize: password handling (restores a dropped regression) ──────

#[test]
fn start_organize_auto_unlocks_an_encrypted_input_via_a_seeded_pass_rule() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let paths = test_paths(&temp);
    support::seed_pass_rule(
        &paths,
        "organize-encrypted.zip",
        "correct-horse-battery-staple",
    );
    let backend: std::sync::Arc<dyn ArchiveBackend> =
        std::sync::Arc::new(FakeEncryptedOrganizeBackend {
            correct_password: "correct-horse-battery-staple".to_string(),
        });
    let app = bootstrap_with_paths(&temp, paths, Some(backend));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("organize-encrypted.zip");
    std::fs::write(
        &input,
        b"placeholder content, the fake backend ignores real bytes",
    )
    .unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
            })
            .await
            .expect("start_organize must be accepted");

        loop {
            let event = tokio::time::timeout(Duration::from_secs(15), receiver.recv())
                .await
                .expect("operation event must arrive within 15s")
                .expect("operation event channel must not close");
            if event.operation_id != operation_id {
                continue;
            }
            match event.state {
                OperationState::Challenge { .. } => {
                    panic!("a seeded matching pass rule must unlock without ever prompting")
                }
                OperationState::Completed { .. } => break,
                OperationState::Failed { error } => panic!("unexpected failure: {error:?}"),
                _ => {}
            }
        }
    });

    assert!(destination.join("organize-encrypted.zip").exists());
}

#[test]
fn start_organize_with_no_matching_pass_rule_raises_a_challenge_the_correct_response_resolves() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let backend: std::sync::Arc<dyn ArchiveBackend> =
        std::sync::Arc::new(FakeEncryptedOrganizeBackend {
            correct_password: "correct-horse-battery-staple".to_string(),
        });
    let app = bootstrap_app_ex(&temp, Some(backend));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("organize-challenge.zip");
    std::fs::write(
        &input,
        b"placeholder content, the fake backend ignores real bytes",
    )
    .unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
            })
            .await
            .expect("start_organize must be accepted");

        let challenge = wait_for_challenge(&mut receiver, operation_id).await;
        let Challenge::Password { id, attempt, .. } = challenge else {
            panic!("expected a Password challenge")
        };
        assert_eq!(attempt, 1);

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id,
                value: SecretInput::new("correct-horse-battery-staple".to_string()),
            },
        )
        .await
        .expect("responding with the correct password must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(messages.iter().any(|m| m.contains("1 succeeded")));
    });

    assert!(destination.join("organize-challenge.zip").exists());
}

#[test]
fn start_organize_a_wrong_password_response_raises_another_challenge_then_the_correct_one_proceeds()
{
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let backend: std::sync::Arc<dyn ArchiveBackend> =
        std::sync::Arc::new(FakeEncryptedOrganizeBackend {
            correct_password: "correct-horse-battery-staple".to_string(),
        });
    let app = bootstrap_app_ex(&temp, Some(backend));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("organize-retry.zip");
    std::fs::write(
        &input,
        b"placeholder content, the fake backend ignores real bytes",
    )
    .unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
            })
            .await
            .expect("start_organize must be accepted");

        let first = wait_for_challenge(&mut receiver, operation_id).await;
        let Challenge::Password {
            id: first_id,
            attempt: first_attempt,
            ..
        } = first
        else {
            panic!("expected a Password challenge")
        };
        assert_eq!(first_attempt, 1);

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id: first_id,
                value: SecretInput::new("wrong-guess".to_string()),
            },
        )
        .await
        .expect("responding to the pending challenge must be accepted");

        let second = wait_for_challenge(&mut receiver, operation_id).await;
        let Challenge::Password {
            id: second_id,
            attempt: second_attempt,
            ..
        } = second
        else {
            panic!("expected a second Password challenge")
        };
        assert_eq!(second_attempt, 2);
        assert_ne!(second_id, first_id, "each challenge gets its own id");

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id: second_id,
                value: SecretInput::new("correct-horse-battery-staple".to_string()),
            },
        )
        .await
        .expect("responding with the correct password must be accepted");

        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(messages.iter().any(|m| m.contains("1 succeeded")));
    });

    assert!(destination.join("organize-retry.zip").exists());
}

/// Pins Important 3's fix (dry-run preview metadata matching the real
/// run's) on the one code path that actually exercises it: every other
/// organize test's fixture name carries no DLsite product code, so
/// `resolve_metadata`'s metadata branch never ran in this suite before.
/// Seeds a real `ProductMetadata` row (placeholder product code
/// `RJ123456`, this codebase's established anonymized-fixture
/// convention -- see e.g. `crates/core/src/utilities/dlsite.rs`'s own
/// tests) through the app's real `LibraryService`, then asserts the
/// dry-run preview's reported output path and the real run's actual
/// produced path are the exact same path, computed independently here
/// rather than parsed back out of either message. This also pins the
/// duplicated dest-path computation the reviewer flagged
/// (`pack_organize_input` vs `preview_organize_input`, the post-review
/// split of what used to be `organize_one_input`/`preview_one_input`):
/// if those two ever computed the sanitized stem differently in the
/// future, this test's independently-computed `expected_dest_path`
/// would stop matching one of them.
#[test]
fn start_organize_dry_run_preview_path_matches_the_real_runs_output_path_when_metadata_exists() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let library_service = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed for a freshly bootstrapped app")
        .core_services
        .library_service
        .clone()
        .expect("library_service must be composed for a freshly bootstrapped app");
    // Chosen deliberately free of any character `sanitize_title` could
    // plausibly rewrite (no punctuation beyond plain spaces), so the
    // expected stem below does not depend on guessing the sanitizer's
    // exact default filter configuration.
    let title = "Placeholder Test Title";
    let mut metadata = ProductMetadata::new(MetadataSource::DLSite, "RJ123456");
    metadata.title = Some(title.to_string());
    library_service
        .save_metadata(&metadata)
        .expect("seeding test metadata must succeed");

    // Filename carries the same placeholder product code the seeded
    // metadata is keyed by -- `resolve_metadata` detects it via
    // `detect_dlsite_code`, exactly as the real quick action did.
    let input = temp.path().join("[RJ123456] Placeholder Game.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");
    let expected_dest_path = destination.join(format!("{title}.zip"));

    let preview_message = runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input.clone()],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: true,
                archive_session_id: None,
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
        messages
            .into_iter()
            .find(|m| m.contains("organize via rule"))
            .expect("expected a preview message")
    });
    assert!(
        preview_message.contains(&expected_dest_path.display().to_string()),
        "expected the preview to report {expected_dest_path:?}, got: {preview_message:?}"
    );
    assert!(
        !expected_dest_path.exists(),
        "dry_run must never create the destination"
    );

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
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

    assert!(
        expected_dest_path.exists(),
        "the real run must have written its output to the exact path the preview reported"
    );
}

/// Facade-wide consistency for the `resolve_metadata` fix: Organize
/// resolves metadata through this crate's own duplicate
/// (`processing_ops::resolve_metadata`); Pipeline resolves it through
/// `arclain_core::execute_pipeline`'s own copy
/// (`executor.rs::resolve_metadata`, fixed separately -- see that
/// function's own regression test,
/// `executor::tests::resolve_metadata_recovers_the_seeded_title_not_just_the_detected_product_code`,
/// in `arclain_core`). Both copies had the identical JSON-shape bug and
/// both are now fixed the same way, but they remain two separate
/// functions in two separate crates (deliberately not deduplicated this
/// round -- flagged as a follow-up) -- this test is the guard that
/// proves they still agree on the same title-based stem for the
/// identical seeded metadata, rather than trusting that two
/// independently-fixed copies stay in sync by construction. Uses
/// Pipeline's `Folder` artifact mode (not Convert/Archive mode): both
/// `OutputArtifact` variants resolve through the exact same
/// `resolve_metadata`/`stem_from` chain in `arclain_core`
/// (`run_one`/`PipelineOutput::{resolve_with_metadata,
/// resolve_folder_with_metadata}` both compute the identical stem), so
/// `Folder` mode proves the same thing `Convert`'s `Archive` mode would
/// without needing a real, working 7-Zip installed to observe it.
#[test]
fn start_organize_and_start_pipeline_agree_on_the_metadata_driven_stem() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let library_service = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed for a freshly bootstrapped app")
        .core_services
        .library_service
        .clone()
        .expect("library_service must be composed for a freshly bootstrapped app");
    let title = "Placeholder Test Title";
    let mut metadata = ProductMetadata::new(MetadataSource::DLSite, "RJ123456");
    metadata.title = Some(title.to_string());
    library_service
        .save_metadata(&metadata)
        .expect("seeding test metadata must succeed");

    // Two distinct input files (organize and pipeline must not race on
    // the same source path), both carrying the same seeded product code
    // so both naming paths detect the identical metadata row.
    let organize_input = temp.path().join("[RJ123456] Placeholder Game organize.zip");
    std::fs::write(&organize_input, b"placeholder content for hashing").unwrap();
    let pipeline_input = build_zip_fixture(
        temp.path(),
        "[RJ123456] Placeholder Game pipeline.zip",
        &[("data.bin", b"alpha-content")],
    );

    let organize_destination = temp.path().join("organize-out");
    let pipeline_destination = temp.path().join("pipeline-out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![organize_input],
                destination: organize_destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
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

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: PipelineInputsDto::Files {
                    paths: vec![pipeline_input],
                },
                destination: PipelineDestinationDto::Folder {
                    path: pipeline_destination.clone(),
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
        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        assert!(messages.iter().any(|m| m.contains("1 succeeded")));
    });

    assert!(
        organize_destination.join(format!("{title}.zip")).exists(),
        "organize must have named its output after the seeded title"
    );
    assert!(
        pipeline_destination.join(title).exists(),
        "pipeline must resolve the identical title-based stem for the same seeded metadata -- \
         if this fails while organize's own file above exists, the two resolve_metadata copies \
         have drifted out of sync"
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

/// Cancelling a Convert mid-batch stops every input that had not
/// started, and leaves no partial output behind for the one that had.
///
/// The `list`/`extract_all` gate makes this deterministic rather than a
/// race: the operation is cancelled only once the first input's
/// extraction is provably under way (its "Processing" progress message
/// has been seen), and the extraction is released only afterwards.
///
/// What "no partial output" means here is exactly what the operation
/// documents, no more: an input already in flight is *not* interrupted
/// (`execute_pipeline` has no mid-file cancellation hook -- the
/// pre-facade UI carried the same limitation, in its own words), so this
/// asserts on the destination rather than pretending the in-flight file
/// stops. `StagedOutput` stages beside the destination and only promotes
/// a verified artifact, so a run that ends without committing must leave
/// the destination directory as empty as it found it -- no half-written
/// archive and no orphaned staging file.
#[test]
fn start_convert_cancellation_stops_unstarted_inputs_and_leaves_no_partial_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();

    // Unique fixture filenames -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let gated_input = build_zip_fixture(
        temp.path(),
        "convert-cancel-first.zip",
        &[("data.bin", b"alpha-content")],
    );
    let unstarted_input = build_zip_fixture(
        temp.path(),
        "convert-cancel-second.zip",
        &[("data.bin", b"beta-content")],
    );

    let (backend, release) = FakeExtractBackend::gated(gated_input.clone());
    let app = bootstrap_app_ex(&temp, Some(backend));
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_convert(ConvertRequest {
                inputs: vec![gated_input.clone(), unstarted_input.clone()],
                destination: destination.clone(),
                format: "zip".to_string(),
                flatten: false,
            })
            .await
            .expect("start_convert must be accepted");

        wait_for_message_containing(
            &mut receiver,
            operation_id,
            "Processing convert-cancel-first",
        )
        .await;

        app.cancel_operation(operation_id)
            .await
            .expect("cancel_operation must succeed while the operation is still running");

        // Only now let the first input's (already in-flight, and not
        // interruptible mid-file) extraction proceed.
        release.send(()).expect("release the gated extraction");

        let (_messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(terminal, OperationState::Cancelled);
    });

    // The unstarted input must never have been touched, and nothing
    // partial may be left where the outputs would have gone.
    let written: Vec<String> = std::fs::read_dir(&destination)
        .map(|dir| {
            dir.filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !written
            .iter()
            .any(|name| name.contains("convert-cancel-second")),
        "an input that had not started yet must never be processed after cancellation;          destination held: {written:?}"
    );
    assert!(
        written.is_empty(),
        "a cancelled run must leave no committed or staged artifact behind;          destination held: {written:?}"
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
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
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
                inputs: PipelineInputsDto::Files { paths: vec![input] },
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
fn start_pipeline_runs_an_ad_hoc_step_list_end_to_end_producing_a_folder() {
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
                inputs: PipelineInputsDto::Files { paths: vec![input] },
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps {
                    steps: vec![PipelineStepDto::Flatten {
                        strip_common_prefix: false,
                        max_depth: 1,
                    }],
                    // Explicit `Folder`: keeps this test off the real,
                    // un-overridable 7-Zip pack step (see
                    // `output_artifact_dto`'s own doc comment for why
                    // this is no longer *derived* from the step list).
                    output_artifact: OutputArtifactDto::Folder,
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

/// The other artifact mode Finding 6 restored: an ad-hoc step list with
/// no `Convert` step, but an explicit `output_artifact: Archive`, must
/// still pack into a real zip (the Process page's own documented,
/// intended default) rather than silently landing as a folder. Gated on
/// a real, unaffected 7-Zip the same way
/// `start_convert_real_happy_path_and_overwrite_when_7z_is_available_and_unaffected`
/// is, since `execute_pipeline`'s Archive mode is a hardcoded,
/// un-overridable `SevenZipCli` call with no fake-backend seam.
#[test]
fn start_pipeline_runs_an_ad_hoc_step_list_end_to_end_producing_an_archive_when_7z_is_available_and_unaffected(
) {
    let Some(sevenzip_path) = detect_unaffected_sevenzip() else {
        eprintln!(
            "skipping start_pipeline_runs_an_ad_hoc_step_list_end_to_end_producing_an_archive_when_7z_is_available_and_unaffected: \
             no unaffected real 7-Zip CLI found on this machine"
        );
        return;
    };

    let runtime = foreign_runtime();
    let scratch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    std::fs::create_dir_all(&scratch_root).expect("create test scratch root");
    let temp = tempfile::Builder::new()
        .prefix("pipeline-adhoc-archive-7z-")
        .tempdir_in(&scratch_root)
        .expect("create scratch tempdir");

    let paths = test_paths(&temp);
    support::seed_working_sevenzip_config(&paths, &sevenzip_path);
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");

    // Unique fixture filename -- see the collision comment in
    // `start_convert_preserves_pre_existing_destination_when_it_is_not_a_recognized_prior_output`.
    let input = build_zip_fixture(
        temp.path(),
        "pipeline-adhoc-archive.zip",
        &[("data.bin", b"alpha-content")],
    );
    let destination = temp.path().join("out");
    let output_path = destination.join("pipeline-adhoc-archive.zip");

    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_pipeline(PipelineRequest {
                inputs: PipelineInputsDto::Files { paths: vec![input] },
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Steps {
                    steps: vec![PipelineStepDto::Flatten {
                        strip_common_prefix: false,
                        max_depth: 1,
                    }],
                    output_artifact: OutputArtifactDto::Archive,
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
            },
            "real conversion must complete; messages so far: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("1 succeeded")),
            "expected a summary message reporting 1 success, got: {messages:?}"
        );
    });

    let packed = std::fs::metadata(&output_path)
        .unwrap_or_else(|_| panic!("expected a real archive at {output_path:?}"));
    assert!(
        packed.len() > 0,
        "a Flatten-only ad-hoc pipeline with output_artifact: Archive must still pack a \
         non-empty archive, not silently fall back to a folder"
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
                inputs: PipelineInputsDto::Files { paths: vec![input] },
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
    let paths = test_paths(&temp);
    // A zero-step preset, not `PipelineSpecDto::Steps { steps: vec![] }`
    // (which `PipelineRequest::validate` now rejects) -- see
    // `seed_zero_step_preset`'s own doc comment for why this test needs
    // a genuinely empty step list rather than a harmless-looking
    // single step.
    seed_zero_step_preset(&paths, "test-zero-step-rollback");
    let app = bootstrap_with_paths(
        &temp,
        paths,
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
                inputs: PipelineInputsDto::Files { paths: vec![input] },
                destination: PipelineDestinationDto::Folder {
                    path: destination.clone(),
                },
                pipeline: PipelineSpecDto::Preset {
                    id: "test-zero-step-rollback".to_string(),
                },
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

// ─── organize: applying exactly what was previewed ─────────────────────

/// Seeds a rule whose organized root folder is *metadata-driven*
/// (`[$product_id] $title`), plus a zip profile -- unlike
/// [`seed_rule_and_profile`]'s deliberately metadata-free rule, this one
/// produces a visibly different plan depending on which metadata
/// resolved, which is exactly what the session-binding tests below
/// measure. Returns `(rule_id, profile_id)`.
fn seed_metadata_driven_rule_and_profile(app: &ArclainApp) -> (i64, i64) {
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
        name: "metadata-driven".to_string(),
        priority: 0,
        is_enabled: true,
        trigger: arclain_core::RuleTrigger::default(),
        actions: arclain_core::RuleActions {
            root_folder: Some("[$product_id] $title".to_string()),
            output_name: None,
            move_files: vec![arclain_core::MoveAction {
                pattern: "**".to_string(),
                target: String::new(),
            }],
            use_standard_layout: false,
        },
    };
    let rule_id = organization_service
        .save_domain_rule(&rule)
        .expect("seeding the metadata-driven organization rule must succeed");

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

/// Saves one DLsite library row for the placeholder product code every
/// fixture in this section uses.
fn seed_library_title(app: &ArclainApp, title: &str) {
    let library_service = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed")
        .core_services
        .library_service
        .clone()
        .expect("library_service must be composed for a freshly bootstrapped app");
    let mut metadata = ProductMetadata::new(MetadataSource::DLSite, "RJ123456");
    metadata.title = Some(title.to_string());
    library_service
        .save_metadata(&metadata)
        .expect("seeding library metadata must succeed");
}

/// Opens `path` as an archive session and reports the session id, the
/// same way any frontend gets one.
async fn open_session(app: &ArclainApp, path: &Path) -> arclain_app::ids::ArchiveSessionId {
    let operation_id = app
        .start_open_archive(arclain_app::archive::OpenArchiveRequest {
            source_path: path.to_path_buf(),
            password: None,
        })
        .await
        .expect("start_open_archive must be accepted");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app.operation(operation_id).await.expect("operation exists");
        match snapshot.state {
            OperationState::Completed {
                result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
            } => return snapshot.session_id,
            OperationState::Failed { error } => panic!("archive open failed: {error:?}"),
            _ => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "archive open did not complete within the test deadline"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

/// Writes plugin-reported metadata onto a session exactly the way a
/// plugin's `emit_metadata` host call does -- through the installed
/// `ActiveTabBridge`, the only path that reaches session metadata.
fn report_plugin_title(
    app: &ArclainApp,
    session_id: arclain_app::ids::ArchiveSessionId,
    title: &str,
) {
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

/// THE invariant this task exists for: **what you preview is what you
/// apply.**
///
/// A session whose plugin metadata says one thing while the DLsite
/// library says another is the case where the two metadata sources
/// visibly disagree -- the organized root folder, and the output file's
/// own name, are both functions of whichever resolved. The first half of
/// this test proves the divergence is real (a path-only organize over
/// the very same archive organizes it under the *library* title), and
/// the second proves a session-bound organize applies the plan the
/// preview reported instead.
///
/// Asserted against what the run actually produced, not against a
/// progress message: the fake backend's "pack" writes a marker naming
/// the directories it was handed, which for `execute_organization_plan`
/// is the organized root folder itself.
#[test]
fn a_session_bound_organize_applies_the_previewed_plan_not_the_library_metadata() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_metadata_driven_rule_and_profile(&app);
    seed_library_title(&app, "Library Title");

    // Carries the placeholder product code the library row is keyed by,
    // so a path-only organize resolves the library's title from it.
    let input = temp.path().join("[RJ123456] Placeholder.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();

    // ── the divergence is real: a path-only organize uses the library ──
    let library_destination = temp.path().join("out-library");
    runtime.block_on(async {
        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![input.clone()],
                destination: library_destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: None,
            })
            .await
            .expect("start_organize must be accepted");
        let (_, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
    });
    let library_output = library_destination.join("Library Title.zip");
    assert!(
        library_output.exists(),
        "a path-only organize must resolve metadata through the library"
    );
    assert!(
        std::fs::read_to_string(&library_output)
            .unwrap()
            .contains("[RJ123456] Library Title"),
        "the packed layout must carry the library-titled root folder"
    );

    // ── preview and apply, bound to a session that disagrees ─────────
    let destination = temp.path().join("out");
    let preview = runtime.block_on(async {
        let session_id = open_session(&app, &input).await;
        report_plugin_title(&app, session_id, "Plugin Title");

        let preview = app
            .preview_organize_plan(session_id, rule_id.to_string())
            .await
            .expect("previewing the seeded rule must succeed");

        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                // The session names the archive; supplying it again is
                // refused (see `OrganizeRequest::validate`).
                inputs: vec![],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: Some(session_id),
            })
            .await
            .expect("a session-bound start_organize must be accepted");
        let (messages, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "messages: {messages:?}"
        );
        preview
    });

    assert_eq!(
        preview.root_folder, "[RJ123456] Plugin Title",
        "the preview must plan from the session's own plugin metadata"
    );

    let applied_output = destination.join("Plugin Title.zip");
    assert!(
        applied_output.exists(),
        "the applied run must name its output from the same metadata the preview used"
    );
    assert!(
        !destination.join("Library Title.zip").exists(),
        "nothing in a session-bound organize may fall back to the library"
    );
    assert!(
        std::fs::read_to_string(&applied_output)
            .unwrap()
            .contains(&preview.root_folder),
        "the packed layout must be the exact root folder the preview reported"
    );
}

/// The session's metadata is the plan's metadata *including when there
/// is none*: falling back to the library for a session that reports
/// nothing would apply a plan the user never previewed (the preview
/// would have shown the metadata-less one).
#[test]
fn a_session_with_no_plugin_metadata_never_falls_back_to_the_library() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_metadata_driven_rule_and_profile(&app);
    seed_library_title(&app, "Library Title");

    let input = temp.path().join("[RJ123456] Placeholder.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");

    let preview = runtime.block_on(async {
        let session_id = open_session(&app, &input).await;
        // Deliberately no `report_plugin_title` call.
        let preview = app
            .preview_organize_plan(session_id, rule_id.to_string())
            .await
            .expect("previewing the seeded rule must succeed");

        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: Some(session_id),
            })
            .await
            .expect("a session-bound start_organize must be accepted");
        let (_, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
        preview
    });

    // With no metadata at all there is nothing to expand `$product_id`
    // or `$title` from, so the rule engine leaves both placeholders
    // standing -- an ugly folder name, but the one the preview showed.
    assert_eq!(preview.root_folder, "[$product_id] $title");
    assert!(
        !destination.join("Library Title.zip").exists(),
        "the library title must not leak into a metadata-less session's organize"
    );
    // The output stem falls back to the detected product code, exactly
    // as the metadata-less preview implies.
    let applied_output = destination.join("RJ123456.zip");
    assert!(
        applied_output.exists(),
        "expected a code-stemmed output, found: {:?}",
        std::fs::read_dir(&destination).map(|entries| entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>())
    );
    assert!(
        std::fs::read_to_string(&applied_output)
            .unwrap()
            .contains(&preview.root_folder),
        "the packed layout must be the exact root folder the preview reported"
    );
}

/// The password a session was opened with is reused, so applying an
/// organize to an already-unlocked archive never prompts for it again --
/// the pre-facade panel's own behavior (it reached for the tab's current
/// password before anything else), preserved through the binding rather
/// than through a second password store.
#[test]
fn a_session_bound_organize_reuses_the_password_the_session_was_opened_with() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let backend: std::sync::Arc<dyn ArchiveBackend> =
        std::sync::Arc::new(FakeEncryptedOrganizeBackend {
            correct_password: "session-password-71c3".to_string(),
        });
    let app = bootstrap_app_ex(&temp, Some(backend));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("organize-session-locked.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");

    runtime.block_on(async {
        // Opened *with* the password, exactly as a user who unlocked the
        // archive in the browser would have.
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: input.clone(),
                password: Some(SecretInput::new("session-password-71c3".to_string())),
            })
            .await
            .expect("start_open_archive must be accepted");
        let session_id = loop {
            match app.operation(operation_id).await.unwrap().state {
                OperationState::Completed {
                    result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
                } => break snapshot.session_id,
                OperationState::Failed { error } => panic!("open failed: {error:?}"),
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        };

        let mut receiver = app.subscribe_operations();
        let organize_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: Some(session_id),
            })
            .await
            .expect("a session-bound start_organize must be accepted");

        loop {
            let event = tokio::time::timeout(Duration::from_secs(15), receiver.recv())
                .await
                .expect("operation event must arrive within 15s")
                .expect("operation event channel must not close");
            if event.operation_id != organize_id {
                continue;
            }
            match event.state {
                OperationState::Challenge { .. } => {
                    panic!("the session's own password must unlock this without prompting")
                }
                OperationState::Completed { .. } => break,
                OperationState::Failed { error } => panic!("organize failed: {error:?}"),
                OperationState::Cancelled => panic!("organize was cancelled"),
                _ => {}
            }
        }
    });

    assert!(destination.join("organize-session-locked.zip").exists());
}

/// A session that was closed (or never existed) is a request-level
/// rejection, not an operation that starts and immediately fails.
#[test]
fn start_organize_rejects_an_unknown_archive_session_id() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app(&temp);
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let err = runtime
        .block_on(app.start_organize(OrganizeRequest {
            inputs: vec![],
            destination: temp.path().join("out"),
            profile_id: profile_id.to_string(),
            rule_id: rule_id.to_string(),
            dry_run: false,
            archive_session_id: Some(arclain_app::ids::ArchiveSessionId::from_raw(999_999)),
        }))
        .unwrap_err();

    assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    assert_no_operation_was_registered(&app, &runtime);
}

/// Organizes `input` with `title` reported as this session's plugin
/// metadata, into its own destination, and reports what the run
/// produced there.
fn organize_with_reported_title(
    runtime: &tokio::runtime::Runtime,
    app: &ArclainApp,
    input: &Path,
    destination: &Path,
    rule_id: i64,
    profile_id: i64,
    title: &str,
) -> Vec<PathBuf> {
    runtime.block_on(async {
        let session_id = open_session(app, input).await;
        let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
        bridge.set_session_metadata(
            session_id.into_raw(),
            Some(serde_json::json!({
                "product_id": "RJ123456",
                "source": "dlsite",
                "title": title,
            })),
        );

        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![],
                destination: destination.to_path_buf(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: Some(session_id),
            })
            .await
            .expect("a session-bound start_organize must be accepted");
        let (_, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            },
            "title {title:?} must not fail the operation"
        );
        app.close_archive(session_id).await.expect("close session");
    });

    std::fs::read_dir(destination)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// A plugin writes the title an organized output is named from, and that
/// name is joined onto a directory the operation packs into with no
/// transaction behind it. Titles that are not usable as a file name at
/// all must therefore fall back to the source stem rather than being
/// spliced into a path.
///
/// These are the forms `sanitize_title`'s *default* filter leaves
/// completely intact, so this measures the component check on its own:
/// before it, `".."` produced `"...zip"`, `"trailing."` produced a name
/// whose trailing dot Windows silently rewrites, and `"NUL"` named a
/// device -- on Windows the run would fail outright rather than write
/// an archive.
#[test]
fn a_plugin_title_that_cannot_name_a_file_falls_back_to_the_source_stem() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    for (index, unusable_title) in ["..", ".", "trailing.", "NUL", "lpt9"]
        .into_iter()
        .enumerate()
    {
        // Deliberately carries no detectable product code, so a refused
        // title falls all the way through to the input's own stem -- the
        // arm whose safety comes from `Path::file_stem` itself.
        let input = temp.path().join(format!("unusable-{index}.zip"));
        std::fs::write(&input, b"placeholder content for hashing").unwrap();
        let destination = temp.path().join(format!("out-unusable-{index}"));

        let produced = organize_with_reported_title(
            &runtime,
            &app,
            &input,
            &destination,
            rule_id,
            profile_id,
            unusable_title,
        );

        assert_eq!(
            produced,
            vec![destination.join(format!("unusable-{index}.zip"))],
            "title {unusable_title:?} must fall back to the source stem"
        );
    }
}

/// The containment property, over every shape a hostile title can take:
/// whatever the run writes, it writes inside its own destination, and
/// the sibling directory a traversal would have reached is untouched.
///
/// Two layers hold this up and the test covers both without depending on
/// which one fires. `sanitize_title` neutralizes the separator forms
/// under its default character set -- but that set is *user
/// configuration*, so the guarantee cannot rest on it; the derivation
/// therefore proves every candidate is a single plain component first
/// (`title_filter::plain_file_component`, whose own tests pin what a
/// narrowed filter leaves for it to catch). An end-to-end narrowed-filter
/// case is deliberately absent: the filter cache is process-global, so
/// narrowing it here would leak into every other test in this binary.
#[test]
fn no_hostile_plugin_title_writes_outside_the_organize_destination() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    // A sibling of every destination: what a traversal would reach.
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let guarded = outside.join("do-not-touch.zip");
    std::fs::write(&guarded, b"a file an escape would overwrite").unwrap();

    // Each title is paired with the name it must produce, which also
    // records *which* layer acted on it. `Some(name)`: the title
    // filter's default character set neutralized it into that plain
    // name. `None`: nothing usable survived, so the derivation fell
    // through to the source stem. Asserting the name (rather than only
    // that the file landed inside the destination) is what catches a
    // derivation that quietly collapses many titles onto one name.
    for (index, (hostile_title, neutralized_name)) in [
        ("../outside/do-not-touch", Some(".._outside_do-not-touch")),
        ("..\\outside\\do-not-touch", Some(".._outside_do-not-touch")),
        ("../../..", None),
        ("..", None),
        (
            "C:\\Windows\\System32\\evil",
            Some("C__Windows_System32_evil"),
        ),
        ("\\\\server\\share\\evil", Some("__server_share_evil")),
        ("name:stream", Some("name_stream")),
        ("NUL", None),
        ("   ", None),
        ("trailing.", None),
        ("nul\u{0}byte", Some("nul_byte")),
    ]
    .into_iter()
    .enumerate()
    {
        let input = temp.path().join(format!("hostile-{index}.zip"));
        std::fs::write(&input, b"placeholder content for hashing").unwrap();
        let destination = temp.path().join(format!("out-hostile-{index}"));

        let produced = organize_with_reported_title(
            &runtime,
            &app,
            &input,
            &destination,
            rule_id,
            profile_id,
            hostile_title,
        );

        let expected_stem = match neutralized_name {
            Some(name) => name.to_string(),
            None => format!("hostile-{index}"),
        };
        assert_eq!(
            produced,
            vec![destination.join(format!("{expected_stem}.zip"))],
            "title {hostile_title:?} must produce exactly this name, in its own destination"
        );
    }

    assert_eq!(
        std::fs::read_to_string(&guarded).unwrap(),
        "a file an escape would overwrite",
        "no hostile title may reach a file outside its destination"
    );
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().flatten().count(),
        1,
        "and none may create one there either"
    );
}

/// The regression the round-1 hardening introduced: a metadata blob with
/// no usable title must not collapse onto one shared output name.
///
/// `sanitize_title` substitutes the literal `"untitled"` for a title
/// that sanitizes to nothing, and that sentinel is a perfectly usable
/// file name — so sanitizing a blank title *returns* it instead of
/// falling through, and the second archive organized that way overwrites
/// the first on a destination with no transaction behind it.
///
/// Two archives, both reporting a blank title, into one destination:
/// two files, each named from its own source.
#[test]
fn two_archives_with_no_metadata_title_do_not_collapse_onto_one_output() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);
    let destination = temp.path().join("out-blank");

    for name in ["[RJ000001] First.zip", "[RJ000002] Second.zip"] {
        let input = temp.path().join(name);
        std::fs::write(&input, b"placeholder content for hashing").unwrap();
        organize_with_reported_title(
            &runtime,
            &app,
            &input,
            &destination,
            rule_id,
            profile_id,
            "",
        );
    }

    let mut produced: Vec<String> = std::fs::read_dir(&destination)
        .expect("the destination must exist")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    produced.sort();
    assert_eq!(
        produced,
        vec!["RJ000001.zip".to_string(), "RJ000002.zip".to_string()],
        "each archive must keep its own output name, taken from its detected code"
    );
}

/// The same fallback with nothing to detect either: a blank title and a
/// name carrying no product code land on the source stem.
#[test]
fn a_blank_metadata_title_falls_back_to_the_source_stem() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("no-code-at-all.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out-stem");

    let produced = organize_with_reported_title(
        &runtime,
        &app,
        &input,
        &destination,
        rule_id,
        profile_id,
        "   ",
    );

    assert_eq!(produced, vec![destination.join("no-code-at-all.zip")]);
}

/// The product-code arm is checked the same way. A code is regex-derived
/// and cannot normally carry a separator, so this pins the guarantee
/// rather than a reachable bug: a title that is refused falls through to
/// the code, and the code is proven a plain component before it is used.
#[test]
fn a_refused_title_falls_through_to_a_checked_product_code() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let app = bootstrap_app_ex(&temp, Some(FakeExtractBackend::always_succeeds()));
    let (rule_id, profile_id) = seed_rule_and_profile(&app);

    let input = temp.path().join("[RJ123456] Placeholder.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();
    let destination = temp.path().join("out");

    let produced = organize_with_reported_title(
        &runtime,
        &app,
        &input,
        &destination,
        rule_id,
        profile_id,
        // Unusable whatever the configured filter set does with it.
        "..",
    );

    assert_eq!(
        produced,
        vec![destination.join("RJ123456.zip")],
        "the refused title must fall through to the detected code"
    );
}

/// The binding is a *snapshot*: metadata written after the operation has
/// been registered cannot change the plan it executes. The guarantee is
/// structural (`SessionBinding` owns cloned values and nothing re-reads
/// the session), so this test exists to make a future refactor that
/// re-reads it fail loudly.
///
/// The write lands while the worker is blocked inside its archive
/// listing -- before any plan is built -- so a re-reading implementation
/// would genuinely pick the new title up.
#[test]
fn metadata_written_after_registration_does_not_change_the_executed_plan() {
    let runtime = foreign_runtime();
    let temp = scratch_tempdir();
    let input = temp.path().join("[RJ123456] Placeholder.zip");
    std::fs::write(&input, b"placeholder content for hashing").unwrap();

    let (backend, arm_listing_gate, listing_gate_fired, release_listing) =
        FakeExtractBackend::listing_gated(input.clone());
    let app = bootstrap_app_ex(&temp, Some(backend));
    let (rule_id, profile_id) = seed_metadata_driven_rule_and_profile(&app);
    let destination = temp.path().join("out");

    runtime.block_on(async {
        let session_id = open_session(&app, &input).await;
        report_plugin_title(&app, session_id, "Before Registration");
        // Opening the session listed the archive too; only the organize
        // worker's own listing is gated.
        arm_listing_gate.store(true, std::sync::atomic::Ordering::SeqCst);

        let mut receiver = app.subscribe_operations();
        let operation_id = app
            .start_organize(OrganizeRequest {
                inputs: vec![],
                destination: destination.clone(),
                profile_id: profile_id.to_string(),
                rule_id: rule_id.to_string(),
                dry_run: false,
                archive_session_id: Some(session_id),
            })
            .await
            .expect("a session-bound start_organize must be accepted");

        // Wait until the worker is actually inside the gated listing, so
        // the write below is genuinely mid-run rather than racing the
        // spawn.
        wait_for_message_containing(&mut receiver, operation_id, "Processing").await;

        // A plugin re-fetches and reports a different title while the
        // operation is in flight.
        report_plugin_title(&app, session_id, "After Registration");
        release_listing.send(()).expect("release the gated listing");

        let (_, terminal) = drain_until_terminal(&mut receiver, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: arclain_app::event::OperationResult::None
            }
        );
    });

    assert!(
        listing_gate_fired.load(std::sync::atomic::Ordering::SeqCst),
        "the gate must actually have caught the worker's listing -- otherwise \
         the write below it landed at no particular moment and this test \
         proves nothing"
    );
    let produced = destination.join("Before Registration.zip");
    assert!(
        produced.exists(),
        "the executed plan must use the metadata snapshotted at registration, found: {:?}",
        std::fs::read_dir(&destination).map(|entries| entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>())
    );
    assert!(
        std::fs::read_to_string(&produced)
            .unwrap()
            .contains("[RJ123456] Before Registration"),
        "and so must the layout it packed"
    );
}
