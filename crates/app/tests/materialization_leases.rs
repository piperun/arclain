//! Integration tests for materialization leases: the `start_materialization`
//! lifecycle (Accepted -> Started -> [Challenge ->] Completed{Materialized})
//! plus `renew_materialization`/`release_materialization`/`materialization`/
//! `read_materialization_range`, driven through `ArclainApp`'s public facade
//! the same way a real frontend would.
//!
//! Every test installs a fake `ArchiveBackend` (via
//! `BootstrapConfig::archive_backend_override`) that both lists the
//! archive's entries and serves extraction -- materialization reuses the
//! open session's own backend handle directly (the same one
//! `crate::operations::extract` already established the pattern for), so
//! there is no separate "runner" seam to fake the way extraction's CLI
//! spawning has.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`, for
//! the same reason `archive_sessions.rs`/`extract_operation.rs` use this
//! shape: dropping `ArclainApp` must not happen from inside an async
//! context, so each test builds `app` in sync code, awaits facade calls
//! through one `runtime.block_on` (borrowing `app`, never moving it into
//! the polled future), and lets `app` drop only after `block_on` returns.

mod support;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::challenge::{Challenge, ChallengeResponse};
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::EntryId;
use arclain_app::materialization::{
    MaterializationPurpose, MaterializeRequest, MAX_MATERIALIZATION_READ_BYTES,
};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Bootstraps an `ArclainApp` whose archive opens and extractions are both
/// served by `backend` -- materialization has no separate CLI-runner seam
/// the way extraction does, since it reuses the open session's own backend
/// handle directly (see the module doc comment).
fn bootstrap_app(paths: AppPaths, backend: Arc<dyn arclain_core::ArchiveBackend>) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        archive_backend_override: Some(backend),
        ..Default::default()
    })
    .expect("bootstrap must succeed")
}

/// Same as [`bootstrap_app`], but with a short lease TTL and cleanup
/// interval so a test can observe real expiry cleanup via a bounded poll
/// instead of waiting on the production 5-minute default. The margin
/// between the TTL and the test's own poll deadline (see
/// `expired_leases_are_removed_by_the_background_cleanup_task`) is kept
/// deliberately large (tens of milliseconds vs. several seconds) -- this
/// is still a real-time-based test (expiry is inherently a wall-clock
/// concept, unlike this crate's other timing-sensitive tests, which use a
/// barrier instead), so a generous margin is what actually buys
/// robustness against scheduler jitter under a loaded machine, not a
/// tighter TTL.
///
/// The flip side of that short TTL: from the moment materialization
/// commits, the sweeper may remove the lease at any point >=150ms later,
/// so a test using this bootstrap must never assert pre-expiry disk state
/// after observing the terminal event -- there is no bound on how late
/// that assertion runs on a loaded machine. Prove "content really was on
/// disk" through commit-time guarantees (`commit` canonicalizes the path;
/// `size` is measured from disk) rather than a fresh `exists()` check.
fn bootstrap_app_with_short_lease_lifetime(
    paths: AppPaths,
    backend: Arc<dyn arclain_core::ArchiveBackend>,
) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        archive_backend_override: Some(backend),
        materialization_lease_ttl_override: Some(Duration::from_millis(150)),
        materialization_cleanup_interval_override: Some(Duration::from_millis(20)),
        ..Default::default()
    })
    .expect("bootstrap must succeed")
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

/// One configured extraction attempt: fail with a diagnostic string (the
/// worker classifies password-shaped diagnostics into a retryable
/// `Challenge::Password`, anything else into a hard failure), or succeed by
/// writing the given `(archive-relative path, content)` pairs under
/// whatever destination directory the worker passes in.
enum ExtractAttempt {
    Fail(String),
    Succeed(HashMap<String, Vec<u8>>),
}

fn succeed(files: &[(&str, &[u8])]) -> ExtractAttempt {
    ExtractAttempt::Succeed(
        files
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_vec()))
            .collect(),
    )
}

/// A fake `ArchiveBackend` that both lists a fixed entry set (for
/// `start_open_archive`) and serves extraction from a scripted queue of
/// [`ExtractAttempt`]s (for `start_materialization`) -- mirrors
/// `extract_operation.rs`'s `FakeListBackend` + `ScriptedRunner` combined
/// into one type, since materialization has only one backend to fake, not
/// a separate listing backend and CLI runner.
struct ScriptedBackend {
    entries: Vec<arclain_core::ArchiveEntry>,
    attempts: Mutex<VecDeque<ExtractAttempt>>,
    captured_passwords: Arc<Mutex<Vec<Option<String>>>>,
}

impl ScriptedBackend {
    fn new(entries: Vec<arclain_core::ArchiveEntry>, attempts: Vec<ExtractAttempt>) -> Arc<Self> {
        Arc::new(Self {
            entries,
            attempts: Mutex::new(attempts.into_iter().collect()),
            captured_passwords: Arc::new(Mutex::new(Vec::new())),
        })
    }
}

impl arclain_core::ArchiveBackend for ScriptedBackend {
    fn name(&self) -> &str {
        "scripted"
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
            encrypted: false,
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
        unimplemented!("materialization never selects the whole archive")
    }
    fn extract_files(
        &self,
        _path: &Path,
        _dest: &Path,
        _files: &[String],
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!("the worker always calls extract_files_with_progress, never this")
    }
    fn extract_files_with_progress(
        &self,
        _path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
        _progress: Option<&arclain_core::ProgressCallback>,
        _cancel: Option<&arclain_core::CancellationToken>,
    ) -> anyhow::Result<()> {
        self.captured_passwords
            .lock()
            .unwrap()
            .push(password.map(str::to_string));
        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .expect("test script ran out of configured extraction attempts");
        match attempt {
            ExtractAttempt::Fail(message) => anyhow::bail!("{message}"),
            ExtractAttempt::Succeed(contents) => {
                for path in files {
                    let content = contents.get(path).cloned().unwrap_or_default();
                    let target = dest.join(path);
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(target, content).unwrap();
                }
                Ok(())
            }
        }
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

/// A fake `ArchiveBackend` whose `extract_files_with_progress` blocks on a
/// barrier (so a test can deterministically know extraction has actually
/// started before cancelling), then loops checking the cancellation token
/// it was handed until it becomes true -- modeling a real backend's
/// documented cancellation contract (`CancellationToken`'s own doc: "if the
/// AtomicBool is set to true, the operation should stop") rather than a
/// poll-based abstraction: unlike extraction's CLI-process runner,
/// materialization's `extract_files_with_progress` call is a single
/// blocking call, so cancellation can only be observed by the call itself
/// checking the token and returning.
struct BarrierBackend {
    entries: Vec<arclain_core::ArchiveEntry>,
    start_barrier: Arc<std::sync::Barrier>,
}

impl arclain_core::ArchiveBackend for BarrierBackend {
    fn name(&self) -> &str {
        "barrier"
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
            encrypted: false,
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
        unimplemented!()
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
    fn extract_files_with_progress(
        &self,
        _path: &Path,
        _dest: &Path,
        _files: &[String],
        _password: Option<&str>,
        _progress: Option<&arclain_core::ProgressCallback>,
        cancel: Option<&arclain_core::CancellationToken>,
    ) -> anyhow::Result<()> {
        self.start_barrier.wait();
        loop {
            if let Some(cancel) = cancel {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    anyhow::bail!("extraction cancelled");
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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

async fn recv_state(
    receiver: &mut tokio::sync::broadcast::Receiver<arclain_app::event::OperationEvent>,
) -> OperationState {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("operation event must arrive within 5s")
        .expect("operation event channel must not close")
        .state
}

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
            panic!("materialization did not reach a terminal state within the test deadline");
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
    directory: &str,
    name: &str,
) -> EntryId {
    let page = app
        .list_entries(
            session_id,
            arclain_app::archive::ListEntriesRequest {
                directory: arclain_app::archive::ArchivePath::parse(directory).unwrap(),
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
        .unwrap_or_else(|| panic!("entry {name:?} not found under {directory:?}"))
        .id
}

/// Extracts the `MaterializationLease` from a terminal `Completed` state,
/// panicking with a descriptive message for any other outcome.
fn expect_materialized(
    state: OperationState,
) -> arclain_app::materialization::MaterializationLease {
    match state {
        OperationState::Completed {
            result: OperationResult::Materialized { lease },
        } => lease,
        OperationState::Failed { error } => {
            panic!("materialization unexpectedly failed: {error:?}")
        }
        other => panic!("expected Completed(Materialized), got {other:?}"),
    }
}

// ============================== Tests ==============================

#[test]
fn materializing_a_file_entry_extracts_it_and_completes_with_a_lease() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("readme.txt")],
        vec![succeed(&[("readme.txt", b"hello, materialized world")])],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "readme.txt").await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::ExternalOpen,
            })
            .await
            .expect("start_materialization must be accepted");

        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        assert_eq!(lease.size, "hello, materialized world".len() as u64);
        assert_eq!(
            std::fs::read(&lease.local_path).unwrap(),
            b"hello, materialized world"
        );
        assert_eq!(lease.local_path.file_name().unwrap(), "readme.txt");

        // Read back via the facade's own query, not just the terminal event.
        let queried = app.materialization(lease.id).await.unwrap();
        assert_eq!(queried, lease);
    });
}

#[test]
fn materializing_a_directory_entry_extracts_the_whole_subtree() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("game/a.txt"), file("game/nested/b.txt")],
        vec![succeed(&[
            ("game/a.txt", b"aaaa"),
            ("game/nested/b.txt", b"bb"),
        ])],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let game_dir_id = entry_id_for(&app, session_id, "", "game").await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![game_dir_id],
                purpose: MaterializationPurpose::DragOut,
            })
            .await
            .unwrap();

        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        assert!(lease.local_path.is_dir());
        assert_eq!(lease.size, 4 + 2);
        assert_eq!(
            std::fs::read(lease.local_path.join("a.txt")).unwrap(),
            b"aaaa"
        );
        assert_eq!(
            std::fs::read(lease.local_path.join("nested").join("b.txt")).unwrap(),
            b"bb"
        );
    });
}

#[test]
fn empty_entry_ids_materializes_the_whole_archive_so_a_root_level_exes_sibling_dll_comes_along() {
    // Regression test for a real behavior gap a review caught: the
    // pre-facade implementation's own directory filter degenerated to
    // "match every entry" for a root-level target (its `dir.is_empty()`
    // branch), so a root-level game executable extracted the *entire*
    // archive, DLLs included. `entry_ids: vec![]` is this facade's own
    // "whole archive" convention (mirroring `ExtractRequest`'s), which is
    // what `file_opener.rs`'s UI-side fallback now requests for exactly
    // this layout.
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("Game.exe"), file("d3d9.dll")],
        vec![succeed(&[
            ("Game.exe", b"exe-bytes"),
            ("d3d9.dll", b"dll-bytes"),
        ])],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: Vec::new(),
                purpose: MaterializationPurpose::ExternalOpen,
            })
            .await
            .unwrap();

        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        assert!(lease.local_path.is_dir());
        assert_eq!(
            std::fs::read(lease.local_path.join("Game.exe")).unwrap(),
            b"exe-bytes"
        );
        assert_eq!(
            std::fs::read(lease.local_path.join("d3d9.dll")).unwrap(),
            b"dll-bytes"
        );
    });
}

#[test]
fn two_or_more_entry_ids_are_rejected_as_invalid_input() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt"), file("b.txt")], vec![]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_a = entry_id_for(&app, session_id, "", "a.txt").await;
        let entry_b = entry_id_for(&app, session_id, "", "b.txt").await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_a, entry_b],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
            }
            other => panic!("expected Failed(InvalidInput), got {other:?}"),
        }
    });
}

#[test]
fn an_unknown_entry_id_fails_with_not_found_and_never_touches_the_backend() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let fabricated = EntryId::from_raw(999_999);

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![fabricated],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound);
            }
            other => panic!("expected Failed(NotFound), got {other:?}"),
        }
    });
}

#[test]
fn a_password_shaped_failure_raises_a_challenge_then_retries_with_the_supplied_password() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("secret.txt")],
        vec![
            ExtractAttempt::Fail("Wrong password for archive".to_string()),
            succeed(&[("secret.txt", b"unlocked")]),
        ],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "secret.txt").await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::ExternalOpen,
            })
            .await
            .unwrap();

        let challenge = loop {
            if let OperationState::Challenge { challenge } = recv_state(&mut events).await {
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

        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);
        assert_eq!(std::fs::read(&lease.local_path).unwrap(), b"unlocked");
    });
}

#[test]
fn a_generic_extraction_failure_cleans_up_the_reserved_lease_directory() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let materialization_dir = paths.cache_dir.join("materialization");
    let backend = ScriptedBackend::new(
        vec![file("a.txt")],
        vec![ExtractAttempt::Fail("disk read error".to_string())],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Backend);
            }
            other => panic!("expected Failed(Backend), got {other:?}"),
        }
    });

    // No leftover reservation directory anywhere under the materialization
    // root -- the RAII guard must have removed it on the failure path.
    let leftover = std::fs::read_dir(&materialization_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        leftover, 0,
        "a failed materialization must not leave its reserved directory behind"
    );
}

#[test]
fn cancellation_stops_the_running_extraction_and_cleans_up_the_lease_directory() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let materialization_dir = paths.cache_dir.join("materialization");
    let start_barrier = Arc::new(std::sync::Barrier::new(2));
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(BarrierBackend {
        entries: vec![file("a.txt")],
        start_barrier: start_barrier.clone(),
    });
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::DragOut,
            })
            .await
            .unwrap();

        // Deterministic rendezvous: once this returns, the worker has
        // called into the backend and is blocked there, proven "running"
        // before we cancel it below.
        let barrier_for_wait = start_barrier.clone();
        tokio::task::spawn_blocking(move || barrier_for_wait.wait())
            .await
            .unwrap();

        app.cancel_operation(operation_id)
            .await
            .expect("cancelling a running materialization must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(terminal, OperationState::Cancelled);
    });

    // Bounded poll (the backend's own cancellation-detection loop sleeps
    // 5ms between checks, so the directory removal lands shortly after the
    // `Cancelled` transition, not necessarily before this scope's own
    // synchronous check runs).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let leftover = std::fs::read_dir(&materialization_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        if leftover == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("a cancelled materialization must not leave its lease directory behind");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn release_removes_the_lease_and_its_directory_and_is_idempotent() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![succeed(&[("a.txt", b"x")])]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);
        assert!(lease.local_path.exists());

        app.release_materialization(lease.id).await.unwrap();
        assert!(!lease.local_path.exists());
        let error = app.materialization(lease.id).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);

        // Idempotent: releasing again must still succeed.
        app.release_materialization(lease.id).await.unwrap();
    });
}

#[test]
fn renew_extends_the_leases_expiry() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![succeed(&[("a.txt", b"x")])]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Edit,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        let new_expiry = app.renew_materialization(lease.id).await.unwrap();

        assert!(
            new_expiry >= lease.expires_at_unix_ms,
            "renewal must not shorten the lease's expiry"
        );
        let queried = app.materialization(lease.id).await.unwrap();
        assert_eq!(queried.expires_at_unix_ms, new_expiry);
    });
}

#[test]
fn renewing_or_releasing_an_unknown_lease_id_is_not_found_for_renew_but_a_no_op_for_release() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![]);
    let app = bootstrap_app(paths, backend);
    let bogus = arclain_app::ids::MaterializationLeaseId::from_raw(999_999);

    runtime.block_on(async {
        let error = app.renew_materialization(bogus).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);

        let error = app.materialization(bogus).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);

        // Releasing a never-issued id is a documented idempotent success.
        app.release_materialization(bogus).await.unwrap();
    });
}

#[test]
fn expired_leases_are_removed_by_the_background_cleanup_task() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![succeed(&[("a.txt", b"x")])]);
    let app = bootstrap_app_with_short_lease_lifetime(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);
        // Deliberately NO `lease.local_path.exists()` precondition here:
        // the 150ms TTL countdown began at commit time, so any test-side
        // disk check placed before the removal poll is racing the very
        // 20ms sweeper this test enables -- on a loaded machine the sweep
        // can legitimately win that race before this line runs. What that
        // assertion used to guard (a vacuous pass where nothing was ever
        // on disk) is already guaranteed by construction without touching
        // the clock: `MaterializationStore::commit` canonicalizes
        // `local_path` -- which fails on a nonexistent path -- before the
        // terminal event can carry the lease at all, and `size` below is
        // measured by the worker from the extracted bytes on disk.
        assert_eq!(
            lease.size, 1,
            "the scripted one-byte payload must actually have been extracted to disk"
        );

        // Bounded poll for the real background cleanup task (150ms TTL,
        // 20ms sweep interval) to notice and remove it -- not an
        // unconditional wait: this converges within a few hundred
        // milliseconds when the implementation is correct, and only
        // reaches the (generously wide) deadline if cleanup is actually
        // broken.
        //
        // Polls the *directory's own* disk state directly, not
        // `app.materialization(lease.id)`'s in-memory lookup: the store's
        // `sweep_expired` removes its map entry and then removes the
        // directory from disk as two separate steps, so a poll that
        // breaks out as soon as the map entry is gone can race ahead of
        // the directory actually being removed -- exactly the "the map
        // said gone, but the real resource was not yet reclaimed" failure
        // mode this workspace's own `archive_sessions.rs` leak test had to
        // learn to avoid the same way (poll the thing you actually care
        // about, not a proxy for it).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !lease.local_path.exists() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "expired lease's directory was never removed by the background cleanup task"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // The in-memory record must be gone too, by this point.
        assert!(app.materialization(lease.id).await.is_err());
    });
}

#[test]
fn application_shutdown_removes_every_outstanding_lease_directory() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![succeed(&[("a.txt", b"x")])]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    let local_path = runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::ExternalOpen,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);
        assert!(lease.local_path.exists());

        app.shutdown().await.unwrap();
        lease.local_path
    });

    assert!(
        !local_path.exists(),
        "ArclainApp::shutdown must remove every outstanding lease's directory"
    );
}

#[test]
fn read_materialization_range_returns_bounded_slices_and_handles_eof() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("a.txt")],
        vec![succeed(&[("a.txt", b"0123456789")])],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        assert_eq!(
            app.read_materialization_range(lease.id, 3, 4)
                .await
                .unwrap(),
            b"3456"
        );
        // Offset at EOF: empty, not an error.
        assert_eq!(
            app.read_materialization_range(lease.id, 10, 5)
                .await
                .unwrap(),
            Vec::<u8>::new()
        );
        // Offset past EOF: empty, not an error.
        assert_eq!(
            app.read_materialization_range(lease.id, 1000, 5)
                .await
                .unwrap(),
            Vec::<u8>::new()
        );
        // A request that overlaps EOF returns only the remaining bytes.
        assert_eq!(
            app.read_materialization_range(lease.id, 8, 100)
                .await
                .unwrap(),
            b"89"
        );

        // Rejected above the maximum bound.
        let error = app
            .read_materialization_range(lease.id, 0, MAX_MATERIALIZATION_READ_BYTES + 1)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);

        // Released lease: NotFound.
        app.release_materialization(lease.id).await.unwrap();
        let error = app
            .read_materialization_range(lease.id, 0, 1)
            .await
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    });
}

#[test]
fn concurrent_reads_of_the_same_lease_all_succeed() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(
        vec![file("a.txt")],
        vec![succeed(&[("a.txt", b"concurrent-readers-payload")])],
    );
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
            })
            .await
            .unwrap();
        let lease = expect_materialized(wait_for_terminal(&app, operation_id).await);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let app = app.clone();
            let id = lease.id;
            handles.push(tokio::spawn(async move {
                app.read_materialization_range(id, 0, 10).await.unwrap()
            }));
        }
        for handle in handles {
            assert_eq!(handle.await.unwrap(), b"concurrent");
        }
    });
}

#[test]
fn exactly_one_terminal_event_is_ever_published_for_a_successful_materialization() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = ScriptedBackend::new(vec![file("a.txt")], vec![succeed(&[("a.txt", b"x")])]);
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session_with_entries(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_materialization(MaterializeRequest {
                session_id,
                entry_ids: vec![entry_id],
                purpose: MaterializationPurpose::Preview,
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
        assert_eq!(ours.kind, OperationKind::Materialize);
    });
}
