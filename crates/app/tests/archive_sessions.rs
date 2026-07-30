//! Integration tests for archive sessions and read operations: the
//! `start_open_archive` operation, `close_archive`/`list_entries`/
//! `archive_snapshot` immediate queries, and the interactive password
//! challenge/response loop -- all driven through `ArclainApp`'s public
//! facade, the same way a real frontend would.
//!
//! Real ZIP fixtures (built with the `zip` crate in a `tempfile::TempDir`)
//! exercise backend selection, indexing, sorting, and open progress
//! end to end through the real `BackendSelector`. Encrypted/password
//! behavior instead uses `BootstrapConfig::archive_backend_override` with
//! a small deterministic fake backend defined in this file: real header-
//! encryption behavior is backend/version-specific and would make tests
//! built on a real encrypted fixture flaky across machines, so these never
//! depend on one.
//!
//! `crates/app/src/archive/session.rs` and `crates/app/src/runtime/
//! archive_ops.rs` carry the more exhaustive crate-internal unit-test
//! coverage of sort/filter/pagination semantics and the password
//! retry/auto-match branching in isolation; this file's job is proving
//! those pieces are wired together correctly behind the public API
//! (real bootstrap, the async operation registry, the challenge/response
//! round trip, and session-id validation).
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`:
//! `ArclainApp` owns its own Tokio runtime (see the crate's runtime/
//! executor rules), and dropping it -- which happens implicitly at the
//! end of each test, when `app` goes out of scope -- must not happen from
//! inside an async context, or Tokio panics ("Cannot drop a runtime in a
//! context where blocking is not allowed"). Each test instead builds `app`
//! in plain sync code, awaits facade calls through one `runtime.block_on`
//! per test (borrowing `app`, never moving it into the polled future), and
//! lets `app` drop only after `block_on` has returned -- matching
//! `crates/app/tests/bootstrap.rs`'s own established pattern for the same
//! reason.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use arclain_app::archive::{
    ArchivePath, EntrySortKey, ListEntriesRequest, OpenArchiveRequest, SortDirection,
};
use arclain_app::challenge::{Challenge, ChallengeResponse, SecretInput};
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::ArchiveSessionId;
use arclain_app::{ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

/// Bootstraps an `ArclainApp` against an isolated temp profile, with a
/// working (dummy-path) 7-Zip so `BackendSelector::select` never fails
/// its own internal `SevenZipCli::detect` fallback probe -- every native
/// backend's fallback chain calls this unconditionally, even when the
/// fallback itself is never exercised.
fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
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

/// Builds a ZIP fixture at `dir/name` containing `entries` (archive-
/// relative path -> content), each with a fixed, known modification
/// timestamp so sort-by-Modified tests are deterministic regardless of
/// wall-clock time.
fn build_zip_fixture(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let modified = zip::DateTime::from_date_and_time(2024, 1, 15, 10, 0, 0)
        .expect("construct a fixed zip fixture timestamp");
    let options = zip::write::SimpleFileOptions::default().last_modified_time(modified);
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
    path
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

/// Reads events until one is NOT a `Progress` tick (progress ticks are
/// not part of this task's characterization -- only the coarse lifecycle
/// states are asserted on).
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

/// Polls `app.operation(operation_id)` until it reaches `Completed`,
/// returning the `ArchiveOpened` snapshot. Bounded so a real bug (the
/// operation getting stuck) fails the test instead of hanging the suite.
async fn wait_for_archive_opened(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app
            .operation(operation_id)
            .await
            .expect("operation must exist");
        match snapshot.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return snapshot,
            OperationState::Failed { error } => {
                panic!("archive open unexpectedly failed: {error:?}")
            }
            OperationState::Cancelled => panic!("archive open was unexpectedly cancelled"),
            _ if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("archive open did not complete within the test deadline"),
        }
    }
}

#[test]
fn opening_a_real_zip_completes_with_an_indexed_snapshot() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive_path = build_zip_fixture(
        temp.path(),
        "fixture.zip",
        &[
            ("readme.txt", b"hello" as &[u8]),
            ("game/Game.exe", b"binary-content"),
            ("game/data/save.dat", b"01234567890123456789"),
        ],
    );

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path.clone(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");

        let snapshot = wait_for_archive_opened(&app, operation_id).await;
        assert_eq!(snapshot.archive_type, "zip");
        assert_eq!(snapshot.source_path, archive_path);
        assert_eq!(
            snapshot.entry_count, 5,
            "3 files (readme.txt, game/Game.exe, game/data/save.dat) + 2 synthesized folders (game, game/data)"
        );
        assert_eq!(snapshot.total_uncompressed_size, 5 + 14 + 20);

        let session_id = snapshot.session_id;
        let root_page = app
            .list_entries(
                session_id,
                ListEntriesRequest {
                    directory: ArchivePath::root(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .await
            .expect("list_entries at root must succeed");
        let root_names: Vec<&str> = root_page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(root_names, ["game", "readme.txt"]);

        let nested_page = app
            .list_entries(
                session_id,
                ListEntriesRequest {
                    directory: ArchivePath::parse("game").unwrap(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .await
            .expect("list_entries in a nested directory must succeed");
        let nested_names: Vec<&str> = nested_page.entries.iter().map(|e| e.name.as_str()).collect();
        // Case-insensitive alphabetical: "data" < "game.exe" ('d' < 'g'),
        // matching the pre-facade UI's own case-insensitive Name sort --
        // not a "folders after files" grouping (there is no such rule).
        assert_eq!(nested_names, ["data", "Game.exe"]);
    });
}

#[test]
fn open_progress_reports_accepted_then_started_then_completed_in_order() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive_path = build_zip_fixture(temp.path(), "fixture.zip", &[("a.txt", b"x")]);

    runtime.block_on(async {
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path,
                password: None,
            })
            .await
            .unwrap();

        // The first event this subscriber sees for our operation must be
        // Accepted (subscribed before the call, and `begin` publishes
        // Accepted synchronously under the same lock it inserts the
        // record with -- see `OperationRegistry::begin`).
        let mut saw_accepted = false;
        let mut saw_started = false;
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("event must arrive")
                .expect("channel must not close");
            if event.operation_id != operation_id {
                continue;
            }
            match event.state {
                OperationState::Accepted => {
                    assert!(!saw_started, "Accepted must precede Started");
                    saw_accepted = true;
                }
                OperationState::Started => {
                    assert!(saw_accepted, "Started must be preceded by Accepted");
                    saw_started = true;
                }
                OperationState::Completed {
                    result: OperationResult::ArchiveOpened { .. },
                } => {
                    assert!(
                        saw_accepted && saw_started,
                        "Completed must follow Accepted and Started"
                    );
                    break;
                }
                OperationState::Failed { error } => panic!("unexpected failure: {error:?}"),
                _ => {}
            }
        }
    });
}

#[test]
fn list_entries_close_and_snapshot_reject_a_reconstructed_unknown_session_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    // Never returned by this app instance -- a purely reconstructed id,
    // as if round-tripped through `ArchiveSessionId::from_raw` from a
    // stale bridge payload or persisted UI state.
    let unknown = ArchiveSessionId::from_raw(999_999);

    runtime.block_on(async {
        let list_error = app
            .list_entries(
                unknown,
                ListEntriesRequest {
                    directory: ArchivePath::root(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(list_error.kind, ApplicationErrorKind::NotFound);

        let snapshot_error = app.archive_snapshot(unknown).await.unwrap_err();
        assert_eq!(snapshot_error.kind, ApplicationErrorKind::NotFound);

        let close_error = app.close_archive(unknown).await.unwrap_err();
        assert_eq!(close_error.kind, ApplicationErrorKind::NotFound);
    });
}

#[test]
fn closing_a_session_makes_every_subsequent_query_not_found() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive_path = build_zip_fixture(temp.path(), "fixture.zip", &[("a.txt", b"x")]);

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path,
                password: None,
            })
            .await
            .unwrap();
        let snapshot = wait_for_archive_opened(&app, operation_id).await;
        let session_id = snapshot.session_id;

        app.close_archive(session_id)
            .await
            .expect("close must succeed once");

        let second_close = app.close_archive(session_id).await.unwrap_err();
        assert_eq!(second_close.kind, ApplicationErrorKind::NotFound);
        let snapshot_error = app.archive_snapshot(session_id).await.unwrap_err();
        assert_eq!(snapshot_error.kind, ApplicationErrorKind::NotFound);
    });
}

#[test]
fn a_detected_multipart_archive_first_part_fails_as_unsupported_not_opened() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    // `.partN.rar` is detected purely by filename (no sibling-file
    // existence check, unlike the plain `.rar`/`.zip` sequence formats) --
    // see `arclain_core::archive::MultiPartArchive::detect`.
    let first_part = temp.path().join("game.part1.rar");
    std::fs::write(&first_part, b"not a real archive, only the name matters").unwrap();

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: first_part,
                password: None,
            })
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = app.operation(operation_id).await.unwrap();
            match snapshot.state {
                OperationState::Failed { error } => {
                    assert_eq!(error.kind, ApplicationErrorKind::Unsupported);
                    break;
                }
                OperationState::Completed { .. } => {
                    panic!("a multi-part archive must not open directly")
                }
                _ if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                _ => panic!("multipart rejection did not happen within the test deadline"),
            }
        }
    });
}

/// Deterministic fake backend for the password/challenge integration
/// tests: succeeds only when given the exact correct password, and
/// otherwise fails with a password-shaped error -- regardless of whether
/// no password, or a wrong one, was supplied. Installed via
/// `BootstrapConfig::archive_backend_override`, which every archive open
/// in that bootstrap uses instead of real extension-based selection.
struct FakeEncryptedBackend {
    correct_password: String,
}

fn fake_backend_info() -> arclain_core::ArchiveInfo {
    arclain_core::ArchiveInfo {
        archive_path: PathBuf::new(),
        archive_kind: arclain_core::archive::ArchiveKind::Zip,
        entries: vec![arclain_core::ArchiveEntry {
            path: "secret.txt".to_string(),
            size: 5,
            packed_size: 5,
            modified: None,
            is_dir: false,
            encrypted: true,
            crc32: None,
        }],
        encrypted: true,
        headers_encrypted: false,
        encryption_method: Some("fake".to_string()),
    }
}

impl arclain_core::ArchiveBackend for FakeEncryptedBackend {
    fn name(&self) -> &str {
        "fake-encrypted"
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
        password: Option<&str>,
    ) -> anyhow::Result<arclain_core::ArchiveInfo> {
        match password {
            Some(candidate) if candidate == self.correct_password => Ok(fake_backend_info()),
            _ => Err(anyhow::anyhow!("Wrong password for archive")),
        }
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

fn bootstrap_app_with_fake_backend(temp: &tempfile::TempDir, correct_password: &str) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeEncryptedBackend {
        correct_password: correct_password.to_string(),
    });
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed")
}

#[test]
fn a_wrong_password_raises_another_challenge_then_the_correct_one_completes() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_fake_backend(&temp, "correct-horse-battery-staple");
    let fake_path = temp.path().join("fake-encrypted.zip");

    runtime.block_on(async {
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: fake_path,
                password: None,
            })
            .await
            .unwrap();

        // First challenge: attempt 1.
        let first_challenge = loop {
            if let OperationState::Challenge { challenge } =
                recv_non_progress_state(&mut events).await
            {
                break challenge;
            }
        };
        let Challenge::Password {
            id: first_id,
            attempt: first_attempt,
            ..
        } = first_challenge
        else {
            panic!("expected a Password challenge");
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

        // Second challenge: attempt 2, after the wrong guess.
        let second_challenge = loop {
            if let OperationState::Challenge { challenge } =
                recv_non_progress_state(&mut events).await
            {
                break challenge;
            }
        };
        let Challenge::Password {
            id: second_id,
            attempt: second_attempt,
            ..
        } = second_challenge
        else {
            panic!("expected a second Password challenge");
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

        let snapshot = wait_for_archive_opened(&app, operation_id).await;
        assert_eq!(snapshot.entry_count, 1);
    });
}

#[test]
fn cancelling_while_a_password_challenge_is_pending_ends_the_operation_as_cancelled() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_fake_backend(&temp, "correct-horse-battery-staple");
    let fake_path = temp.path().join("fake-encrypted.zip");

    runtime.block_on(async {
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: fake_path,
                password: None,
            })
            .await
            .unwrap();

        loop {
            if matches!(
                recv_non_progress_state(&mut events).await,
                OperationState::Challenge { .. }
            ) {
                break;
            }
        }

        app.cancel_operation(operation_id)
            .await
            .expect("cancelling a pending-challenge operation must be accepted");

        let snapshot = app.operation(operation_id).await.unwrap();
        assert_eq!(snapshot.state, OperationState::Cancelled);

        // Answering the now-cancelled operation's stale challenge must be
        // rejected, not silently accepted.
        let response = app
            .respond_to_challenge(
                operation_id,
                ChallengeResponse::Password {
                    id: arclain_app::ids::ChallengeId::from_raw(1),
                    value: SecretInput::new("too-late".to_string()),
                },
            )
            .await;
        assert!(
            response.is_err(),
            "a cancelled operation has no pending challenge to answer"
        );
    });
}

/// A backend whose `list()` blocks (on a real OS thread -- it always runs
/// inside `spawn_blocking`) until the test releases it. Lets a test land a
/// cancellation deterministically while the archive-open worker's blocking
/// `list()` call is still in flight, instead of racing real wall-clock
/// timing against it.
struct SlowBackend {
    started: mpsc::Sender<()>,
    // `ArchiveBackend` requires `Sync` (it is used behind `Arc<dyn
    // ArchiveBackend>`); `mpsc::Receiver` is `Send` but not `Sync`, so it
    // needs a mutex even though only ever accessed from the single
    // `spawn_blocking` thread that calls `list()`.
    proceed: std::sync::Mutex<mpsc::Receiver<()>>,
}

impl arclain_core::ArchiveBackend for SlowBackend {
    fn name(&self) -> &str {
        "slow"
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
        let _ = self.started.send(());
        let _ = self
            .proceed
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5));
        Ok(arclain_core::ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: arclain_core::archive::ArchiveKind::Zip,
            entries: vec![arclain_core::ArchiveEntry {
                path: "a.txt".to_string(),
                size: 1,
                packed_size: 1,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
            encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        })
    }
    fn extract_all(&self, _p: &Path, _d: &Path, _pw: Option<&str>) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_files(
        &self,
        _p: &Path,
        _d: &Path,
        _files: &[String],
        _pw: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _p: &Path,
        _d: &Path,
        _dir_path: &str,
        _pw: Option<&str>,
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

/// Reproduces the exact race a cancel-during-open can hit: `list()` is
/// still blocked (still running inside `spawn_blocking`) when
/// `cancel_operation` is called, so the operation reaches `Cancelled`
/// well before the backend call it raced against ever returns. Once
/// released, that call reports success as normal -- proving the fix is
/// that a *later* success can no longer insert a session or dispatch a
/// plugin event for an operation the caller already knows was cancelled.
#[test]
fn cancelling_while_the_blocking_list_call_is_still_running_leaves_no_session_behind() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (proceed_tx, proceed_rx) = mpsc::channel::<()>();
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(SlowBackend {
        started: started_tx,
        proceed: std::sync::Mutex::new(proceed_rx),
    });
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");
    let slow_path = temp.path().join("slow.zip");

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: slow_path,
                password: None,
            })
            .await
            .unwrap();

        // Wait until the backend's blocking `list()` call has actually
        // started (not merely scheduled), so the cancel below reliably
        // lands while it is still in flight rather than before
        // `spawn_blocking` even begins running it.
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the slow backend's list() must start within 5s");

        app.cancel_operation(operation_id)
            .await
            .expect("cancelling while list() is in flight must be accepted");

        // Only now release the blocked backend call, well after the
        // cancellation has already been recorded.
        let _ = proceed_tx.send(());

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = app.operation(operation_id).await.unwrap();
            match snapshot.state {
                OperationState::Cancelled => break,
                OperationState::Completed { .. } => {
                    panic!("an open cancelled while list() was still running must not complete")
                }
                _ if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                _ => panic!("operation did not settle to Cancelled within the test deadline"),
            }
        }

        // `ArchiveSessionStore::next_id` is a field seeded fresh at
        // construction (not a process-wide `static` shared with every
        // other test's own store -- see that field's own doc comment for
        // why that distinction matters here specifically), so this
        // bootstrap's store mints ids starting at 1, and this test opens
        // nothing else before or during the race. A store-size
        // introspection is not reachable through the public facade at
        // all (this file only sees what `ArclainApp` exports), so a
        // small range of the first few ids is probed instead of just id
        // 1, hedging against any single-id assumption about exactly
        // where the counter starts.
        //
        // The operation reaching `Cancelled` (checked above) does NOT by
        // itself prove the worker task has finished running its own
        // post-`list()` logic: `cancel_operation` records that state
        // synchronously, well before `proceed_tx.send(())` even runs
        // above, so a single probe taken right after seeing `Cancelled`
        // would race the worker's own remaining work rather than actually
        // wait for it -- exactly the failure mode that made an earlier
        // version of this test pass even with the fix reverted. Instead,
        // poll for the session's *absence* across a bounded window: with
        // the fix, absence is permanent (a correctly cancelled open never
        // creates a session at all), so polling longer only increases
        // confidence and can never flip a true negative into a false one;
        // a regression that leaks a session finishes creating it within
        // low milliseconds of the release above (indexing an empty entry
        // list is not real work), so a one-second window polled every
        // 20ms reliably observes it.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            for candidate in 1..=3u64 {
                let probe = app
                    .archive_snapshot(ArchiveSessionId::from_raw(candidate))
                    .await;
                assert_eq!(
                    probe.unwrap_err().kind,
                    ApplicationErrorKind::NotFound,
                    "cancelling during the blocking list() call must not leave session {candidate} reachable"
                );
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

#[test]
fn a_seeded_pass_rule_unlocks_automatically_without_ever_raising_a_challenge() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    support::seed_pass_rule(
        &paths,
        "auto-unlock-fixture.zip",
        "correct-horse-battery-staple",
    );
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(FakeEncryptedBackend {
        correct_password: "correct-horse-battery-staple".to_string(),
    });
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed");
    let fake_path = temp.path().join("auto-unlock-fixture.zip");

    runtime.block_on(async {
        let mut events = app.subscribe_operations();

        let _operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: fake_path,
                password: None,
            })
            .await
            .unwrap();

        loop {
            match recv_non_progress_state(&mut events).await {
                OperationState::Challenge { .. } => {
                    panic!("a seeded matching pass rule must unlock without ever prompting")
                }
                OperationState::Completed {
                    result: OperationResult::ArchiveOpened { .. },
                } => break,
                OperationState::Failed { error } => panic!("unexpected failure: {error:?}"),
                _ => {}
            }
        }
    });
}

#[test]
fn recent_operations_and_operation_kind_report_open_archive() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive_path = build_zip_fixture(temp.path(), "fixture.zip", &[("a.txt", b"x")]);

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path,
                password: None,
            })
            .await
            .unwrap();
        wait_for_archive_opened(&app, operation_id).await;

        let recent = app.recent_operations(10).await.unwrap();
        let ours = recent
            .iter()
            .find(|snapshot| snapshot.operation_id == operation_id)
            .expect("our operation must appear in recent_operations");
        assert_eq!(ours.kind, OperationKind::OpenArchive);
    });
}

/// The whole-archive path list an organize panel's "Original" tree is
/// built from: every file, in the same stable path-sorted order the
/// paged read model uses, and none of the directories the entry index
/// synthesizes from those paths.
#[test]
fn archive_file_paths_lists_every_file_path_sorted_and_without_synthesized_folders() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    // Deliberately written in a non-sorted order, and with nested paths
    // whose parent directories the archive never lists explicitly.
    let archive_path = build_zip_fixture(
        temp.path(),
        "fixture.zip",
        &[
            ("wrapper/readme.txt", b"read me"),
            ("wrapper/data/pack.bin", b"packed"),
            ("wrapper/Game.exe", b"executable"),
            ("top.txt", b"top level"),
        ],
    );

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path,
                password: None,
            })
            .await
            .unwrap();
        let snapshot = wait_for_archive_opened(&app, operation_id).await;

        let paths = app
            .archive_file_paths(snapshot.session_id)
            .await
            .expect("an open session must report its file paths");

        assert_eq!(
            paths,
            vec![
                "top.txt".to_string(),
                "wrapper/Game.exe".to_string(),
                "wrapper/data/pack.bin".to_string(),
                "wrapper/readme.txt".to_string(),
            ],
            "every file, path-sorted, and no synthesized `wrapper`/`wrapper/data` folder"
        );

        // The same paths `list_entries` reports as files, so the panel's
        // two views of one archive cannot disagree.
        assert_eq!(
            paths.len(),
            snapshot.entry_count as usize - 2,
            "the two synthesized ancestor folders are counted by entry_count but never listed here"
        );
    });
}

#[test]
fn archive_file_paths_rejects_a_reconstructed_unknown_session_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let unknown = ArchiveSessionId::from_raw(999_999);

    let error = runtime
        .block_on(app.archive_file_paths(unknown))
        .unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

/// The whole-archive listing the browser's tree/info/plugin consumers are
/// built from: every file *and* every directory (synthesized ancestors
/// included), in depth-first tree order, with the directory rows carrying
/// the kind flag and recursive aggregates a tree panel needs.
#[test]
fn list_all_entries_reports_the_whole_tree_including_synthesized_directories() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let archive_path = build_zip_fixture(
        temp.path(),
        "fixture.zip",
        &[
            ("wrapper/readme.txt", b"read me"),
            ("wrapper/data/pack.bin", b"packed"),
            ("wrapper/Game.exe", b"executable"),
            ("top.txt", b"top level"),
        ],
    );

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: archive_path,
                password: None,
            })
            .await
            .unwrap();
        let snapshot = wait_for_archive_opened(&app, operation_id).await;

        let inventory = app
            .list_all_entries(snapshot.session_id)
            .await
            .expect("an open session must report its whole entry tree");

        assert_eq!(inventory.session_id, snapshot.session_id);
        assert_eq!(inventory.revision, snapshot.revision);
        assert_eq!(
            inventory.entries.len() as u64,
            snapshot.entry_count,
            "the inventory and the snapshot must agree on what an entry is"
        );
        assert_eq!(
            inventory
                .entries
                .iter()
                .map(|dto| dto.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "top.txt",
                "wrapper",
                "wrapper/data",
                "wrapper/data/pack.bin",
                "wrapper/Game.exe",
                "wrapper/readme.txt",
            ],
            "depth-first tree order: each directory's name-sorted children, parents first"
        );

        let wrapper = inventory
            .entries
            .iter()
            .find(|dto| dto.path.as_str() == "wrapper")
            .unwrap();
        assert_eq!(wrapper.kind, arclain_app::archive::EntryKind::Directory);
        assert_eq!(
            wrapper.uncompressed_size,
            (b"read me".len() + b"packed".len() + b"executable".len()) as u64,
            "a directory row aggregates every descendant file recursively"
        );

        // The rows carry the same session-minted ids the paged read model
        // hands out -- one id space, so a consumer can hand either's ids
        // to extract/delete/materialize.
        let root_page = app
            .list_entries(
                snapshot.session_id,
                ListEntriesRequest {
                    directory: ArchivePath::root(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: u32::MAX,
                },
            )
            .await
            .unwrap();
        for page_row in &root_page.entries {
            let inventory_row = inventory
                .entries
                .iter()
                .find(|dto| dto.id == page_row.id)
                .expect("every paged row must appear in the inventory under the same id");
            assert_eq!(inventory_row, page_row);
        }
    });
}

#[test]
fn list_all_entries_rejects_a_reconstructed_unknown_session_id() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let unknown = ArchiveSessionId::from_raw(999_999);

    let error = runtime.block_on(app.list_all_entries(unknown)).unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}
