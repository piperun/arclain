//! Integration tests for extraction as an application operation: the
//! `start_extract` lifecycle (Accepted -> Started -> Progress ->
//! collision/password challenges -> exactly one terminal state), driven
//! through `ArclainApp`'s public facade the same way a real frontend
//! would.
//!
//! Every test installs a deterministic fake `ExtractRunner` via
//! `BootstrapConfig::extract_runner_override` -- unlike listing (which
//! the real native ZIP backend can do without any external tool), real
//! extraction through the CLI runner genuinely needs a real 7-Zip
//! executable to produce real files, which would make these tests
//! dependent on what happens to be installed on the machine running
//! them. Real 7z/ZIP/RAR extraction is exercised manually instead (see
//! this task's report).
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! for the same reason `archive_sessions.rs` uses this shape: dropping
//! `ArclainApp` must not happen from inside an async context (Tokio
//! panics), so each test builds `app` in sync code, awaits facade calls
//! through one `runtime.block_on` (borrowing `app`, never moving it into
//! the polled future), and lets `app` drop only after `block_on` returns.

mod support;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::challenge::{Challenge, ChallengeResponse};
use arclain_app::error::{ApplicationError, ApplicationErrorKind};
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::EntryId;
use arclain_app::operations::extract::{
    ExtractPlan, ExtractProgressEvent, ExtractRunner, ExtractSelection, RunningExtraction,
};
use arclain_app::operations::{CollisionPolicy, ExtractRequest};
use arclain_app::{ArclainApp, BootstrapConfig};

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

/// Bootstraps an `ArclainApp` whose extraction operation spawns through
/// `runner` instead of the real 7-Zip CLI, and whose archive opens are
/// served by `backend` instead of real extension-based selection.
fn bootstrap_app(
    temp: &tempfile::TempDir,
    backend: Arc<dyn arclain_core::ArchiveBackend>,
    runner: Arc<dyn ExtractRunner>,
) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: Some(runner),
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed")
}

/// Minimal in-memory `ArchiveBackend` for opening a session to extract
/// from. `list` always succeeds (with or without a password), so tests
/// drive `start_open_archive` without any password-challenge dance of
/// their own -- extraction needing a password `list()` never did (per-
/// file encrypted data under unencrypted headers) is modeled purely
/// through `ArchiveEntry::encrypted`, independent of whatever `list`
/// itself required.
struct FakeListBackend {
    entries: Vec<arclain_core::ArchiveEntry>,
}

impl arclain_core::ArchiveBackend for FakeListBackend {
    fn name(&self) -> &str {
        "fake-list"
    }
    fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
        arclain_core::archive::BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
        Ok(arclain_core::archive::ArchiveKind::Zip)
    }
    fn list(
        &self,
        _path: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<arclain_core::ArchiveInfo> {
        Ok(arclain_core::ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: arclain_core::archive::ArchiveKind::Zip,
            entries: self.entries.clone(),
            encrypted: self.entries.iter().any(|e| e.encrypted),
            headers_encrypted: false,
            encryption_method: None,
        })
    }
    fn extract_all(
        &self,
        _path: &Path,
        _dest: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!("extraction goes through ExtractRunner in these tests, never this trait")
    }
    fn extract_files(
        &self,
        _path: &Path,
        _dest: &Path,
        _files: &[String],
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _path: &Path,
        _dest: &Path,
        _dir_path: &str,
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn create_archive(
        &self,
        _dest: &Path,
        _files: &[PathBuf],
        _format: &str,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn read_text_file(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _password: Option<&str>,
    ) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn delete_files(&self, _archive: &Path, _files: &[String]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _content: &str,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn convert_to_7z(
        &self,
        _source: &arclain_core::Archive,
        _dest: &Path,
        _temp_dir: &Path,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn crc32_of_entry(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _password: Option<&str>,
    ) -> anyhow::Result<String> {
        unimplemented!()
    }
}

fn file(path: &str) -> arclain_core::ArchiveEntry {
    arclain_core::ArchiveEntry {
        path: path.to_string(),
        size: 1,
        packed_size: 1,
        modified: None,
        is_dir: false,
        encrypted: false,
        crc32: None,
    }
}

fn encrypted_file(path: &str) -> arclain_core::ArchiveEntry {
    arclain_core::ArchiveEntry {
        encrypted: true,
        ..file(path)
    }
}

async fn recv_state(
    receiver: &mut tokio::sync::broadcast::Receiver<arclain_app::event::OperationEvent>,
) -> OperationState {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("operation event must arrive within 5s")
        .expect("operation event channel must not close")
        .state
}

async fn recv_non_progress_state(
    receiver: &mut tokio::sync::broadcast::Receiver<arclain_app::event::OperationEvent>,
) -> OperationState {
    loop {
        let state = recv_state(receiver).await;
        if !matches!(state, OperationState::Progress { .. }) {
            return state;
        }
    }
}

/// Polls `app.operation(operation_id)` until it reaches a terminal
/// state, returning it. Bounded so a stuck operation fails the test
/// instead of hanging the suite.
async fn wait_for_terminal(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> OperationState {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app
            .operation(operation_id)
            .await
            .expect("operation must exist");
        if matches!(
            snapshot.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return snapshot.state;
        }
        if std::time::Instant::now() >= deadline {
            panic!("extraction did not reach a terminal state within the test deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn open_session_with_entries(
    app: &ArclainApp,
    path: &Path,
) -> arclain_app::ids::ArchiveSessionId {
    let operation_id = app
        .start_open_archive(OpenArchiveRequest {
            source_path: path.to_path_buf(),
            password: None,
        })
        .await
        .expect("start_open_archive must be accepted");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        match snapshot.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return snapshot.session_id,
            OperationState::Failed { error } => {
                panic!("archive open unexpectedly failed: {error:?}")
            }
            _ if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("archive open did not complete within the test deadline"),
        }
    }
}

async fn entry_id_for(
    app: &ArclainApp,
    session_id: arclain_app::ids::ArchiveSessionId,
    name: &str,
) -> EntryId {
    let page = app
        .list_entries(
            session_id,
            arclain_app::archive::ListEntriesRequest {
                directory: arclain_app::archive::ArchivePath::root(),
                sort_key: arclain_app::archive::EntrySortKey::Name,
                sort_direction: arclain_app::archive::SortDirection::Ascending,
                name_filter: None,
                offset: 0,
                limit: 1000,
            },
        )
        .await
        .expect("list_entries must succeed");
    page.entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("entry {name:?} not found in listing"))
        .id
}

// ============================= Fakes =============================

/// A snapshot of one `spawn()` call's `ExtractPlan`, captured for
/// assertions -- password redacted to a bool the same way `ExtractPlan`
/// itself redacts it in `Debug`.
#[derive(Debug, Clone)]
struct CapturedPlan {
    destination: PathBuf,
    files: Option<Vec<String>>,
    password: Option<String>,
}

impl From<&ExtractPlan> for CapturedPlan {
    fn from(plan: &ExtractPlan) -> Self {
        Self {
            destination: plan.destination().to_path_buf(),
            files: match plan.selection() {
                ExtractSelection::WholeArchive => None,
                ExtractSelection::Files(files) => Some(files.clone()),
            },
            password: plan.password().map(str::to_string),
        }
    }
}

/// One configured extraction attempt's outcome.
enum ScriptedAttempt {
    Fail(ApplicationErrorKind),
    Succeed,
    /// Succeeds, additionally writing `files` (relative path -> bytes)
    /// under the plan's destination -- lets a test exercise
    /// `CollisionPolicy::Rename`'s staging-then-move finalization
    /// end-to-end against a real filesystem.
    SucceedWriting(Vec<(&'static str, &'static [u8])>),
}

/// Deterministic, scriptable fake `ExtractRunner`: each `spawn()` call
/// captures its plan and consumes the next configured attempt from a
/// queue, so a test can script a full retry sequence (e.g. wrong
/// password, then correct).
struct ScriptedRunner {
    tool_available: bool,
    attempts: Mutex<VecDeque<ScriptedAttempt>>,
    captured: Arc<Mutex<Vec<CapturedPlan>>>,
}

impl ScriptedRunner {
    fn new(attempts: Vec<ScriptedAttempt>) -> (Arc<Self>, Arc<Mutex<Vec<CapturedPlan>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(Self {
            tool_available: true,
            attempts: Mutex::new(attempts.into_iter().collect()),
            captured: captured.clone(),
        });
        (runner, captured)
    }

    fn tool_missing() -> Arc<Self> {
        Arc::new(Self {
            tool_available: false,
            attempts: Mutex::new(VecDeque::new()),
            captured: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

impl ExtractRunner for ScriptedRunner {
    fn tool_available(&self) -> bool {
        self.tool_available
    }

    fn spawn(&self, plan: &ExtractPlan) -> Result<Box<dyn RunningExtraction>, ApplicationError> {
        self.captured.lock().unwrap().push(CapturedPlan::from(plan));
        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .expect("test script ran out of configured extraction attempts");
        let outcome = match attempt {
            ScriptedAttempt::Fail(kind) => Err(ApplicationError::new(kind, "scripted failure")),
            ScriptedAttempt::Succeed => Ok(()),
            ScriptedAttempt::SucceedWriting(files) => {
                for (relative, content) in files {
                    let target = plan.destination().join(relative);
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(target, content).unwrap();
                }
                Ok(())
            }
        };
        Ok(Box::new(ScriptedRunning {
            progress_sent: false,
            outcome: Some(outcome),
        }))
    }
}

struct ScriptedRunning {
    progress_sent: bool,
    outcome: Option<Result<(), ApplicationError>>,
}

impl RunningExtraction for ScriptedRunning {
    fn poll_progress(&mut self) -> Option<ExtractProgressEvent> {
        if self.progress_sent {
            None
        } else {
            self.progress_sent = true;
            Some(ExtractProgressEvent {
                percent: 50,
                message: Some("scripted progress".to_string()),
            })
        }
    }

    fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>> {
        self.outcome.take()
    }

    fn kill(&mut self) {}
}

/// Fake `ExtractRunner` whose spawned "process" never completes on its
/// own -- only cancellation ends it. `spawn()` rendezvous on `barrier`
/// (a 2-party `std::sync::Barrier`) before returning, so a test can
/// synchronize on it too and know deterministically that the operation
/// has reached "spawned, about to run" before the test cancels it --
/// removing the race between "did the worker even start" and "the test
/// already cancelled".
struct BarrierRunner {
    barrier: Arc<std::sync::Barrier>,
    kill_called: Arc<std::sync::atomic::AtomicBool>,
}

impl ExtractRunner for BarrierRunner {
    fn tool_available(&self) -> bool {
        true
    }

    fn spawn(&self, _plan: &ExtractPlan) -> Result<Box<dyn RunningExtraction>, ApplicationError> {
        self.barrier.wait();
        Ok(Box::new(BarrierRunning {
            kill_called: self.kill_called.clone(),
        }))
    }
}

struct BarrierRunning {
    kill_called: Arc<std::sync::atomic::AtomicBool>,
}

impl RunningExtraction for BarrierRunning {
    fn poll_progress(&mut self) -> Option<ExtractProgressEvent> {
        None
    }

    fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>> {
        None
    }

    fn kill(&mut self) {
        self.kill_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

// ============================== Tests ==============================

#[test]
fn selected_entry_extraction_completes_and_the_runner_receives_only_that_file() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt"), file("b.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![ScriptedAttempt::Succeed]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "a.txt").await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![entry_id],
                destination: destination.clone(),
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .expect("start_extract must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].files, Some(vec!["a.txt".to_string()]));
        assert_eq!(captured[0].destination, destination);
    });
}

#[test]
fn whole_archive_extraction_passes_no_explicit_file_list() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt"), file("b.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![ScriptedAttempt::Succeed]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination.clone(),
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
        assert_eq!(captured.lock().unwrap()[0].files, None);
    });
}

#[test]
fn cancellation_kills_the_running_process_and_ends_as_cancelled() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let kill_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(BarrierRunner {
        barrier: barrier.clone(),
        kill_called: kill_called.clone(),
    });
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        // Deterministic rendezvous: once this returns, the worker has
        // called `runner.spawn()` and is now polling a process that
        // never finishes on its own -- proven "running", not merely
        // "accepted", before we cancel it below.
        let barrier_for_wait = barrier.clone();
        tokio::task::spawn_blocking(move || barrier_for_wait.wait())
            .await
            .unwrap();

        app.cancel_operation(operation_id)
            .await
            .expect("cancelling a running extraction must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(terminal, OperationState::Cancelled);

        // Bounded poll for the worker noticing cancellation on its next
        // tick and killing the fake process -- not a race: the operation
        // is already recorded Cancelled above, this only proves the
        // worker *itself* cooperated rather than the registry bookkeeping
        // alone.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if kill_called.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("cancellation was recorded but the running process was never killed");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

#[test]
fn a_relative_destination_is_rejected_before_the_runner_is_ever_invoked() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: PathBuf::from("relative/dest"),
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
            }
            other => panic!("expected Failed(InvalidInput), got {other:?}"),
        }
        assert!(
            captured.lock().unwrap().is_empty(),
            "runner must never be invoked"
        );
    });
}

#[test]
fn a_destination_that_is_an_existing_file_is_rejected() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination_is_a_file = temp.path().join("not-a-directory");
    std::fs::write(&destination_is_a_file, b"oops").unwrap();

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination_is_a_file,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
            }
            other => panic!("expected Failed(InvalidInput), got {other:?}"),
        }
        assert!(captured.lock().unwrap().is_empty());
    });
}

#[test]
fn selecting_by_entry_id_can_never_smuggle_a_traversal_path_to_the_runner() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    // The malicious raw entry never gets an `EntryId` at all -- see
    // `EntryIndex::build`'s own characterization -- so no caller can ever
    // reference it through `entry_ids`. A fabricated/never-issued id is
    // the only thing an attacker (or a stale bridge payload) could try.
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("safe.txt"), file("../../evil.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        // Confirm the traversal-shaped entry was indeed never indexed --
        // only "safe.txt" is listed.
        let page = app
            .list_entries(
                session_id,
                arclain_app::archive::ListEntriesRequest {
                    directory: arclain_app::archive::ArchivePath::root(),
                    sort_key: arclain_app::archive::EntrySortKey::Name,
                    sort_direction: arclain_app::archive::SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "safe.txt");

        // A fabricated id (never issued by this session) is the only way
        // to even attempt referencing the never-indexed entry.
        let fabricated = EntryId::from_raw(999_999);
        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![fabricated],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound);
            }
            other => panic!("expected Failed(NotFound), got {other:?}"),
        }
        assert!(
            captured.lock().unwrap().is_empty(),
            "no traversal-shaped (or any) path may ever reach the CLI runner"
        );
    });
}

#[test]
fn a_collision_under_ask_raises_a_challenge_and_declining_skips_only_the_colliding_file() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("keep.txt"), file("collide.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![ScriptedAttempt::Succeed]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("collide.txt"), b"already here").unwrap();

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination.clone(),
                collision_policy: CollisionPolicy::Ask,
            })
            .await
            .unwrap();

        let challenge = loop {
            if let OperationState::Challenge { challenge } =
                recv_non_progress_state(&mut events).await
            {
                break challenge;
            }
        };
        let Challenge::ConfirmOverwrite {
            id: challenge_id,
            destination: challenged_destination,
        } = challenge
        else {
            panic!("expected a ConfirmOverwrite challenge");
        };
        assert_eq!(challenged_destination, destination);

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::ConfirmOverwrite {
                id: challenge_id,
                overwrite: false,
            },
        )
        .await
        .expect("declining the overwrite must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].files, Some(vec!["keep.txt".to_string()]));
    });
}

/// Regression test for a real data-loss bug: `Skip` filtering out every
/// candidate (because all of them already exist at the destination) must
/// never fall through to spawning the CLI runner with an empty file
/// list. An empty explicit file-list argument is, to the CLI, identical
/// to "no filter at all" -- it would extract (and, via the unconditional
/// `-y`, silently overwrite) everything, which is exactly backwards for
/// a policy whose entire point is to leave existing files alone.
#[test]
fn skip_with_every_candidate_colliding_completes_without_ever_spawning() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt"), file("b.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();
    std::fs::write(destination.join("b.txt"), b"already here too").unwrap();

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Skip,
            })
            .await
            .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
        assert!(
            captured.lock().unwrap().is_empty(),
            "the runner must never be invoked when nothing survives the collision filter"
        );
    });

    // The pre-existing files must be untouched -- proof positive that no
    // extraction (least of all an unfiltered whole-archive one) ran.
    assert_eq!(
        std::fs::read(temp.path().join("dest").join("a.txt")).unwrap(),
        b"already here"
    );
    assert_eq!(
        std::fs::read(temp.path().join("dest").join("b.txt")).unwrap(),
        b"already here too"
    );
}

/// The same data-loss shape as the `Skip` case above, reached through
/// `Ask` instead: declining the overwrite when *every* candidate
/// collided leaves nothing to extract, and that must also complete
/// without spawning rather than falling through to "no filter".
#[test]
fn declining_ask_with_every_candidate_colliding_completes_without_ever_spawning() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("a.txt"), b"already here").unwrap();

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Ask,
            })
            .await
            .unwrap();

        let challenge = loop {
            if let OperationState::Challenge { challenge } =
                recv_non_progress_state(&mut events).await
            {
                break challenge;
            }
        };
        let Challenge::ConfirmOverwrite { id, .. } = challenge else {
            panic!("expected a ConfirmOverwrite challenge");
        };

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::ConfirmOverwrite {
                id,
                overwrite: false,
            },
        )
        .await
        .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
        assert!(
            captured.lock().unwrap().is_empty(),
            "the runner must never be invoked when nothing survives the decline"
        );
    });
    assert_eq!(
        std::fs::read(temp.path().join("dest").join("a.txt")).unwrap(),
        b"already here"
    );
}

/// A whole-archive `Skip` (or a declined `Ask`) with no collisions at all
/// still expands to an explicit per-file argument list (there is no
/// "extract everything except these" flag), and a large enough archive's
/// full file list can exceed a single CLI invocation's safe command-line
/// length. Proves the operation splits that list into multiple
/// sequential runner invocations rather than either failing outright or
/// silently truncating the file list.
#[test]
fn whole_archive_skip_with_a_large_file_list_splits_into_multiple_chunked_invocations() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    // 40 entries at ~1000 chars each (~40,000 chars total) comfortably
    // exceeds the 28,000-char per-chunk ceiling with no collisions
    // involved at all -- this is purely a command-line-length split.
    let entries: Vec<arclain_core::ArchiveEntry> = (0..40)
        .map(|i| file(&format!("{i:04}-{}.bin", "x".repeat(995))))
        .collect();
    let expected_names: std::collections::HashSet<String> =
        entries.iter().map(|e| e.path.clone()).collect();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend { entries });
    // One `Succeed` queued per expected chunk; if the operation spawned
    // more invocations than scripted, `ScriptedRunner` panics with "ran
    // out of configured extraction attempts" instead of silently passing.
    let (runner, captured) = ScriptedRunner::new(vec![
        ScriptedAttempt::Succeed,
        ScriptedAttempt::Succeed,
        ScriptedAttempt::Succeed,
    ]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Skip,
            })
            .await
            .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
    });

    let captured = captured.lock().unwrap();
    assert!(
        captured.len() > 1,
        "a file list this large must split across more than one invocation, got {}",
        captured.len()
    );
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for plan in captured.iter() {
        let files = plan
            .files
            .as_ref()
            .expect("a chunk must carry an explicit file list");
        let total_len: usize = files.iter().map(|f| f.len() + 1).sum();
        assert!(
            total_len <= 28_000,
            "one chunk's total argument length {total_len} exceeds the safety ceiling"
        );
        for f in files {
            assert!(
                seen.insert(f.clone()),
                "file {f} extracted by more than one chunk"
            );
        }
    }
    assert_eq!(
        seen, expected_names,
        "every file must be covered exactly once"
    );
}

#[test]
fn a_password_shaped_failure_raises_a_challenge_then_retries_with_the_supplied_password() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    // Headers are not encrypted (list succeeds with no password at all),
    // but this entry's own data is -- the real-world case extraction can
    // hit a password requirement `start_open_archive` never did.
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![encrypted_file("secret.txt")],
    });
    let (runner, captured) = ScriptedRunner::new(vec![
        ScriptedAttempt::Fail(ApplicationErrorKind::PasswordRequired),
        ScriptedAttempt::Succeed,
    ]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        let challenge = loop {
            if let OperationState::Challenge { challenge } =
                recv_non_progress_state(&mut events).await
            {
                break challenge;
            }
        };
        let Challenge::Password { id, attempt, .. } = challenge else {
            panic!("expected a Password challenge");
        };
        assert_eq!(attempt, 1);

        app.respond_to_challenge(
            operation_id,
            ChallengeResponse::Password {
                id,
                value: arclain_app::challenge::SecretInput::new("correct-horse".to_string()),
            },
        )
        .await
        .expect("responding with a password must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2, "one failed attempt, then one retry");
        assert_eq!(captured[0].password, None);
        assert_eq!(captured[1].password, Some("correct-horse".to_string()));
    });
}

#[test]
fn a_missing_cli_tool_fails_immediately_without_ever_spawning() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let runner = ScriptedRunner::tool_missing();
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::ExternalToolMissing);
            }
            other => panic!("expected Failed(ExternalToolMissing), got {other:?}"),
        }
    });
}

#[test]
fn a_generic_cli_exit_failure_fails_the_operation_without_retrying() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let (runner, captured) =
        ScriptedRunner::new(vec![ScriptedAttempt::Fail(ApplicationErrorKind::Backend)]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Backend);
            }
            other => panic!("expected Failed(Backend), got {other:?}"),
        }
        assert_eq!(
            captured.lock().unwrap().len(),
            1,
            "a non-password failure must not retry"
        );
    });
}

#[test]
fn exactly_one_terminal_event_is_ever_published_for_a_successful_extraction() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("a.txt")],
    });
    let (runner, _captured) = ScriptedRunner::new(vec![ScriptedAttempt::Succeed]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination,
                collision_policy: CollisionPolicy::Overwrite,
            })
            .await
            .unwrap();

        wait_for_terminal(&app, operation_id).await;

        let mut terminal_count = 0;
        loop {
            match events.try_recv() {
                Ok(event) if event.operation_id == operation_id => {
                    if matches!(
                        event.state,
                        OperationState::Completed { .. }
                            | OperationState::Cancelled
                            | OperationState::Failed { .. }
                    ) {
                        terminal_count += 1;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        assert_eq!(terminal_count, 1);

        let recent = app.recent_operations(10).await.unwrap();
        let ours = recent
            .iter()
            .find(|snapshot| snapshot.operation_id == operation_id)
            .expect("our operation must appear in recent_operations");
        assert_eq!(ours.kind, OperationKind::Extract);
    });
}

#[test]
fn rename_policy_stages_then_moves_files_renaming_only_on_a_real_collision() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeListBackend {
        entries: vec![file("keep.txt"), file("collide.txt")],
    });
    let (runner, _captured) = ScriptedRunner::new(vec![ScriptedAttempt::SucceedWriting(vec![
        ("keep.txt", b"fresh keep"),
        ("collide.txt", b"fresh collide"),
    ])]);
    let app = bootstrap_app(&temp, backend, runner);
    let archive_path = temp.path().join("archive.zip");
    let destination = temp.path().join("dest");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(destination.join("collide.txt"), b"pre-existing").unwrap();

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination.clone(),
                collision_policy: CollisionPolicy::Rename,
            })
            .await
            .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
    });

    assert_eq!(
        std::fs::read(destination.join("keep.txt")).unwrap(),
        b"fresh keep"
    );
    assert_eq!(
        std::fs::read(destination.join("collide.txt")).unwrap(),
        b"pre-existing",
        "the pre-existing file must never be overwritten by Rename"
    );
    assert_eq!(
        std::fs::read(destination.join("collide (1).txt")).unwrap(),
        b"fresh collide"
    );
    // No leftover staging directory.
    let leftover_hidden_dirs = std::fs::read_dir(&destination)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".arclain-extract-")
        })
        .count();
    assert_eq!(leftover_hidden_dirs, 0);
}

#[test]
fn extract_request_and_collision_policy_are_constructible_and_serialize_snake_case() {
    let request = ExtractRequest {
        session_id: arclain_app::ids::ArchiveSessionId::from_raw(1),
        entry_ids: vec![EntryId::from_raw(2), EntryId::from_raw(3)],
        destination: PathBuf::from("/tmp/out"),
        collision_policy: CollisionPolicy::Skip,
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "session_id": 1,
            "entry_ids": [2, 3],
            "destination": "/tmp/out",
            "collision_policy": "skip",
        })
    );
}
