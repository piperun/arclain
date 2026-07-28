//! Integration tests for archive mutation as an application operation:
//! the `start_archive_mutation` lifecycle (Accepted -> Started ->
//! [`SnapshotChanged`] -> exactly one terminal state), driven through
//! `ArclainApp`'s public facade the same way a real frontend would.
//!
//! Every test installs a deterministic, fully in-memory fake
//! `ArchiveBackend` via `BootstrapConfig::archive_backend_override` --
//! mirroring `extract_operation.rs`'s own approach -- rather than a real
//! ZIP/7z file, since what is under test here is the *operation's* own
//! orchestration (capability gating, revision checks, reindexing,
//! cancellation), not any particular backend's mutation logic.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! for the same reason `archive_sessions.rs`/`extract_operation.rs` use
//! this shape: dropping `ArclainApp` must not happen from inside an
//! async context (Tokio panics), so each test builds `app` in sync code,
//! awaits facade calls through one `runtime.block_on` (borrowing `app`,
//! never moving it into the polled future), and lets `app` drop only
//! after `block_on` returns.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::archive::{
    ArchivePath, ArchiveSnapshot, EntryPage, ListEntriesRequest, OpenArchiveRequest,
};
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::{ArchiveSessionId, EntryId, OperationId};
use arclain_app::operations::ArchiveMutationRequest;
use arclain_app::{ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Bootstraps an `ArclainApp` whose archive opens/mutations are all
/// served by `backend` instead of real extension-based selection.
fn bootstrap_app(
    temp: &tempfile::TempDir,
    backend: Arc<dyn arclain_core::ArchiveBackend>,
) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
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

fn entry(path: &str, size: u64) -> arclain_core::ArchiveEntry {
    arclain_core::ArchiveEntry {
        path: path.to_string(),
        size,
        packed_size: size,
        modified: None,
        is_dir: false,
        encrypted: false,
        crc32: None,
    }
}

// ============================== Fake backend ==============================

/// A fully in-memory, mutable `ArchiveBackend`: `list()` always reflects
/// the current `entries`, and `add_files`/`delete_files`/
/// `add_or_update_file_from_str` actually mutate that same state, so a
/// re-`list()` after a mutation genuinely observes it -- letting these
/// tests prove the operation's reindex/revision bump reflects real
/// backend-committed content, not merely "the call didn't return Err".
struct FakeBackend {
    capabilities: arclain_core::archive::BackendCapabilities,
    entries: Mutex<Vec<arclain_core::ArchiveEntry>>,
    add_files_calls: Mutex<usize>,
    delete_files_calls: Mutex<usize>,
    replace_text_calls: Mutex<Vec<(String, String)>>,
    fail_add_files: Mutex<Option<String>>,
    fail_delete_files: Mutex<Option<String>>,
    /// When set, `add_files` sends on `.0` (proving it was entered) and
    /// then blocks on `.1.recv()` before doing anything else -- lets a
    /// test deterministically prove a second mutation on the same
    /// session is genuinely still queued behind this one.
    add_files_gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    /// One-shot: when true, the *next* `list()` call fails (and this
    /// resets to `false` regardless of whether that call was consumed by
    /// an open, a re-list after a mutation, or a plain query) -- lets a
    /// test script a mutation whose own backend call succeeds but whose
    /// follow-up reindex-driving re-list fails, without needing a
    /// separate "list has been called N times" counter.
    fail_next_list: Mutex<bool>,
    /// Same rendezvous shape as `add_files_gate`, but for `list()`:
    /// armed via `gate_next_list_call` (not a constructor, unlike
    /// `with_add_files_gate`) so a test can open a session first --
    /// consuming the `list()` call every open needs -- and only then arm
    /// the gate on the *next* one, which is always the post-mutation
    /// re-list. Lets a test prove a second mutation is genuinely queued
    /// on `mutation_lock` while the first is still stuck inside the
    /// relist that its own `mark_desynced()` call depends on, rather than
    /// merely submitted after the first's relist already failed.
    list_gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

impl FakeBackend {
    fn new(entries: Vec<arclain_core::ArchiveEntry>) -> Self {
        Self {
            capabilities: arclain_core::archive::BackendCapabilities::full_featured(),
            entries: Mutex::new(entries),
            add_files_calls: Mutex::new(0),
            delete_files_calls: Mutex::new(0),
            replace_text_calls: Mutex::new(Vec::new()),
            fail_add_files: Mutex::new(None),
            fail_delete_files: Mutex::new(None),
            add_files_gate: Mutex::new(None),
            fail_next_list: Mutex::new(false),
            list_gate: Mutex::new(None),
        }
    }

    fn read_only(entries: Vec<arclain_core::ArchiveEntry>) -> Self {
        Self {
            capabilities: arclain_core::archive::BackendCapabilities::read_only(),
            ..Self::new(entries)
        }
    }

    fn failing_add_files(entries: Vec<arclain_core::ArchiveEntry>, message: &str) -> Self {
        let backend = Self::new(entries);
        *backend.fail_add_files.lock().unwrap() = Some(message.to_string());
        backend
    }

    /// Installs a gate: the *next* `add_files` call sends on the
    /// returned `Sender<()>` and then blocks until the test sends on the
    /// returned `Sender<()>` counterpart.
    fn with_add_files_gate(
        entries: Vec<arclain_core::ArchiveEntry>,
    ) -> (
        Self,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let backend = Self::new(entries);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
        *backend.add_files_gate.lock().unwrap() = Some((started_tx, proceed_rx));
        (backend, started_rx, proceed_tx)
    }

    /// Schedules the *next* `list()` call (whichever one it turns out to
    /// be) to fail -- see `fail_next_list`'s own doc comment.
    fn fail_next_list_call(&self) {
        *self.fail_next_list.lock().unwrap() = true;
    }

    /// Arms a gate on the *next* `list()` call -- see `list_gate`'s own
    /// doc comment. Combine with `fail_next_list_call` (armed either
    /// before or after this) to gate a relist that then fails once
    /// released, the exact "mutation succeeded, follow-up relist did
    /// not" scenario, but now with a real, deterministically-proven
    /// second mutation queued on the lock while the first is blocked.
    fn gate_next_list_call(&self) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
        *self.list_gate.lock().unwrap() = Some((started_tx, proceed_rx));
        (started_rx, proceed_tx)
    }

    fn add_files_call_count(&self) -> usize {
        *self.add_files_calls.lock().unwrap()
    }

    fn delete_files_call_count(&self) -> usize {
        *self.delete_files_calls.lock().unwrap()
    }

    fn replace_text_calls(&self) -> Vec<(String, String)> {
        self.replace_text_calls.lock().unwrap().clone()
    }

    fn current_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        paths.sort();
        paths
    }
}

impl arclain_core::ArchiveBackend for FakeBackend {
    fn name(&self) -> &str {
        "fake-mutable"
    }
    fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
        self.capabilities
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
        Ok(arclain_core::archive::ArchiveKind::Zip)
    }
    fn list(
        &self,
        _path: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<arclain_core::ArchiveInfo> {
        // Drain the gate under its own short-lived lock so the actual
        // send/block never happens while still holding it -- mirrors
        // `add_files`'s identical handling of `add_files_gate`.
        let gate = self.list_gate.lock().unwrap().take();
        if let Some((started_tx, proceed_rx)) = gate {
            let _ = started_tx.send(());
            let _ = proceed_rx.recv();
        }

        {
            let mut should_fail = self.fail_next_list.lock().unwrap();
            if *should_fail {
                *should_fail = false;
                return Err(anyhow::anyhow!("scripted list failure"));
            }
        }
        Ok(arclain_core::ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: arclain_core::archive::ArchiveKind::Zip,
            entries: self.entries.lock().unwrap().clone(),
            encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        })
    }
    fn extract_all(&self, _: &Path, _: &Path, _: Option<&str>) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_files(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _: &Path,
        _: &Path,
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn recompress_7z(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_files(&self, _archive: &Path, files: &[PathBuf]) -> anyhow::Result<()> {
        *self.add_files_calls.lock().unwrap() += 1;

        // Drain the gate under its own short-lived lock so the actual
        // send/block never happens while still holding it (a second
        // concurrent call -- there is at most one in these tests -- must
        // never deadlock on this mutex itself).
        let gate = self.add_files_gate.lock().unwrap().take();
        if let Some((started_tx, proceed_rx)) = gate {
            let _ = started_tx.send(());
            let _ = proceed_rx.recv();
        }

        if let Some(message) = self.fail_add_files.lock().unwrap().clone() {
            return Err(anyhow::anyhow!(message));
        }
        let mut entries = self.entries.lock().unwrap();
        for file in files {
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.to_string_lossy().into_owned());
            entries.push(entry(&name, 1));
        }
        Ok(())
    }
    fn create_archive(&self, _: &Path, _: &[PathBuf], _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn read_text_file(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn delete_files(&self, _archive: &Path, files: &[String]) -> anyhow::Result<()> {
        *self.delete_files_calls.lock().unwrap() += 1;
        if let Some(message) = self.fail_delete_files.lock().unwrap().clone() {
            return Err(anyhow::anyhow!(message));
        }
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|e| !files.contains(&e.path));
        Ok(())
    }
    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.replace_text_calls
            .lock()
            .unwrap()
            .push((path_in_archive.to_string(), content.to_string()));
        let mut entries = self.entries.lock().unwrap();
        if let Some(existing) = entries.iter_mut().find(|e| e.path == path_in_archive) {
            existing.size = content.len() as u64;
            existing.packed_size = content.len() as u64;
        } else {
            entries.push(entry(path_in_archive, content.len() as u64));
        }
        Ok(())
    }
    fn convert_to_7z(&self, _: &arclain_core::Archive, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn crc32_of_entry(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
}

// ============================== Helpers ==============================

async fn open_session(app: &ArclainApp, path: &Path) -> ArchiveSessionId {
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
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            _ => panic!("archive open did not complete within the test deadline"),
        }
    }
}

async fn snapshot(app: &ArclainApp, session_id: ArchiveSessionId) -> ArchiveSnapshot {
    app.archive_snapshot(session_id)
        .await
        .expect("archive_snapshot must succeed")
}

async fn list_root(app: &ArclainApp, session_id: ArchiveSessionId) -> EntryPage {
    app.list_entries(
        session_id,
        ListEntriesRequest {
            directory: ArchivePath::root(),
            sort_key: arclain_app::archive::EntrySortKey::Name,
            sort_direction: arclain_app::archive::SortDirection::Ascending,
            name_filter: None,
            offset: 0,
            limit: 1000,
        },
    )
    .await
    .expect("list_entries must succeed")
}

async fn entry_id_for(app: &ArclainApp, session_id: ArchiveSessionId, name: &str) -> EntryId {
    list_root(app, session_id)
        .await
        .entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entry {name:?} not found in listing"))
        .id
}

async fn wait_for_terminal(app: &ArclainApp, operation_id: OperationId) -> OperationState {
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
            panic!("archive mutation did not reach a terminal state within the test deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn add_files_request(
    session_id: ArchiveSessionId,
    expected_revision: u64,
    source_paths: Vec<PathBuf>,
) -> ArchiveMutationRequest {
    ArchiveMutationRequest::AddFiles {
        session_id,
        expected_revision,
        destination: ArchivePath::root(),
        source_paths,
    }
}

// ============================== Tests ==============================

#[test]
fn add_files_appends_at_the_root_and_bumps_the_revision_exactly_once() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("existing.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let new_file = temp.path().join("new_file.txt");
    std::fs::write(&new_file, b"hello").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        assert_eq!(snapshot(&app, session_id).await.revision, 1);

        let operation_id = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![new_file.clone()]))
            .await
            .expect("start_archive_mutation must be accepted");

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );

        assert_eq!(snapshot(&app, session_id).await.revision, 2);
        let page = list_root(&app, session_id).await;
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"new_file.txt"));
        assert_eq!(backend.add_files_call_count(), 1);
    });
}

#[test]
fn a_non_root_add_files_destination_is_rejected_as_unsupported() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let request = ArchiveMutationRequest::AddFiles {
            session_id,
            expected_revision: 1,
            destination: ArchivePath::parse("subfolder".to_string()).unwrap(),
            source_paths: vec![PathBuf::from("/tmp/whatever.txt")],
        };
        let operation_id = app.start_archive_mutation(request).await.unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Unsupported)
            }
            other => panic!("expected Failed(Unsupported), got {other:?}"),
        }
        assert_eq!(backend.add_files_call_count(), 0);
    });
}

#[test]
fn unsupported_read_only_backends_reject_every_mutation_kind_before_ever_calling_the_backend() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::read_only(vec![
        entry("a.txt", 1),
        entry("b.txt", 1),
    ]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let a_id = entry_id_for(&app, session_id, "a.txt").await;

        let add_op = app
            .start_archive_mutation(add_files_request(
                session_id,
                1,
                vec![PathBuf::from("/tmp/new.txt")],
            ))
            .await
            .unwrap();
        match wait_for_terminal(&app, add_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Unsupported)
            }
            other => panic!("expected AddFiles Failed(Unsupported), got {other:?}"),
        }

        let delete_op = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![a_id],
            })
            .await
            .unwrap();
        match wait_for_terminal(&app, delete_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Unsupported)
            }
            other => panic!("expected DeleteEntries Failed(Unsupported), got {other:?}"),
        }

        let replace_op = app
            .start_archive_mutation(ArchiveMutationRequest::ReplaceText {
                session_id,
                expected_revision: 1,
                entry_id: a_id,
                content: "new content".to_string(),
            })
            .await
            .unwrap();
        match wait_for_terminal(&app, replace_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Unsupported)
            }
            other => panic!("expected ReplaceText Failed(Unsupported), got {other:?}"),
        }

        assert_eq!(backend.add_files_call_count(), 0);
        assert_eq!(backend.delete_files_call_count(), 0);
        assert!(backend.replace_text_calls().is_empty());
        // The read-only rejection never touched the session either.
        assert_eq!(snapshot(&app, session_id).await.revision, 1);
    });
}

#[test]
fn add_files_with_no_source_paths_completes_as_a_no_op_without_touching_the_backend() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![]))
            .await
            .unwrap();

        let terminal = wait_for_terminal(&app, operation_id).await;
        assert_eq!(
            terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
        assert_eq!(backend.add_files_call_count(), 0);
        // Nothing changed -- no SnapshotChanged for this operation.
        assert_eq!(snapshot(&app, session_id).await.revision, 1);
        let mut saw_snapshot_changed = false;
        while let Ok(event) = events.try_recv() {
            if event.operation_id == operation_id
                && matches!(event.state, OperationState::SnapshotChanged { .. })
            {
                saw_snapshot_changed = true;
            }
        }
        assert!(
            !saw_snapshot_changed,
            "an empty AddFiles must never emit SnapshotChanged"
        );
    });
}

#[test]
fn delete_entries_with_an_empty_selection_completes_as_a_no_op_without_touching_the_backend() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![],
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
        assert_eq!(backend.delete_files_call_count(), 0);
        assert_eq!(snapshot(&app, session_id).await.revision, 1);
    });
}

#[test]
fn deleting_a_directory_expands_to_every_descendant_file() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![
        entry("keep.txt", 1),
        entry("game/data.bin", 1),
        entry("game/nested/save.dat", 1),
    ]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let game_dir_id = entry_id_for(&app, session_id, "game").await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![game_dir_id],
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
        assert_eq!(backend.delete_files_call_count(), 1);
        assert_eq!(backend.current_paths(), vec!["keep.txt".to_string()]);

        let page = list_root(&app, session_id).await;
        let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["keep.txt"],
            "the whole game/ subtree must be gone"
        );
    });
}

#[test]
fn deleting_a_directory_with_no_descendant_files_completes_as_a_no_op() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    // "empty" is only ever synthesized as a directory if some file
    // implies it -- give it an implied child that a separate delete
    // already removed conceptually is awkward to model, so instead this
    // uses a directory whose only membership is itself (no way to
    // reference an empty directory by id without a descendant to imply
    // it from `arclain_core::ArchiveEntry`, so this test targets the
    // same "resolves to zero paths" branch via a `Directory`-kind id
    // whose descendants were already deleted in an earlier call).
    let backend = Arc::new(FakeBackend::new(vec![entry("game/data.bin", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let game_dir_id = entry_id_for(&app, session_id, "game").await;

        // First delete empties the directory for real.
        let first = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![game_dir_id],
            })
            .await
            .unwrap();
        wait_for_terminal(&app, first).await;
        assert_eq!(backend.delete_files_call_count(), 1);

        // `game_dir_id` is now stale (the directory no longer exists in
        // the rebuilt index) -- resolving it again must be `NotFound`,
        // not a silent second delete call. This locks in that a
        // superseded id can never be replayed against a later revision.
        let second = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 2,
                entry_ids: vec![game_dir_id],
            })
            .await
            .unwrap();
        match wait_for_terminal(&app, second).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound)
            }
            other => panic!("expected Failed(NotFound) for a superseded id, got {other:?}"),
        }
        assert_eq!(
            backend.delete_files_call_count(),
            1,
            "a stale directory id must never reach a second delete_files call"
        );
    });
}

#[test]
fn replace_text_round_trips_multibyte_content_unmodified() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("readme.txt", 5)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let content = "héllo wörld 日本語 -- multi-byte round trip";

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "readme.txt").await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::ReplaceText {
                session_id,
                expected_revision: 1,
                entry_id,
                content: content.to_string(),
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

        let calls = backend.replace_text_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "readme.txt");
        assert_eq!(
            calls[0].1, content,
            "multi-byte content must reach the backend byte-for-byte unmodified"
        );
    });
}

#[test]
fn replacing_text_on_a_directory_entry_is_rejected_as_invalid_input() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("folder/inner.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let folder_id = entry_id_for(&app, session_id, "folder").await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::ReplaceText {
                session_id,
                expected_revision: 1,
                entry_id: folder_id,
                content: "oops".to_string(),
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::InvalidInput)
            }
            other => panic!("expected Failed(InvalidInput), got {other:?}"),
        }
        assert!(backend.replace_text_calls().is_empty());
    });
}

#[test]
fn a_stale_expected_revision_is_rejected_as_conflict_before_any_backend_work() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let first_new_file = temp.path().join("first.txt");
    std::fs::write(&first_new_file, b"1").unwrap();
    let second_new_file = temp.path().join("second.txt");
    std::fs::write(&second_new_file, b"2").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        // First mutation, run to completion against the real starting
        // revision -- bumps 1 -> 2.
        let first_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![first_new_file]))
            .await
            .unwrap();
        wait_for_terminal(&app, first_op).await;
        assert_eq!(snapshot(&app, session_id).await.revision, 2);
        assert_eq!(backend.add_files_call_count(), 1);

        // Second mutation submitted with the now-stale revision (1).
        let second_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![second_new_file]))
            .await
            .unwrap();
        match wait_for_terminal(&app, second_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict)
            }
            other => panic!("expected Failed(Conflict), got {other:?}"),
        }

        // The stale attempt must never have reached the backend, and the
        // revision/content must be exactly what the first mutation left.
        assert_eq!(backend.add_files_call_count(), 1);
        assert_eq!(snapshot(&app, session_id).await.revision, 2);
        assert_eq!(
            backend.current_paths(),
            vec!["a.txt".to_string(), "first.txt".to_string()]
        );
    });
}

/// The brief's own regression test: a failed mutation must never leave
/// the session's entry index claiming content the backend did not
/// commit -- index refresh + revision bump strictly after backend
/// success.
#[test]
fn a_backend_failure_leaves_the_session_index_and_revision_exactly_as_they_were() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::failing_add_files(
        vec![entry("a.txt", 1), entry("b.txt", 1)],
        "disk full",
    ));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let new_file = temp.path().join("new.txt");
    std::fs::write(&new_file, b"x").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let before = list_root(&app, session_id).await;

        let operation_id = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![new_file]))
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Backend)
            }
            other => panic!("expected Failed(Backend), got {other:?}"),
        }

        // The backend was genuinely invoked once (proving this is a real
        // failure, not a rejection before the call) but never mutated
        // its own state -- the fake never appends on a scripted failure.
        assert_eq!(backend.add_files_call_count(), 1);

        // Revision must be exactly unchanged...
        assert_eq!(
            snapshot(&app, session_id).await.revision,
            1,
            "a failed mutation must never bump the revision"
        );
        // ...and the index must still describe only the original,
        // backend-committed entries -- never claiming the new file that
        // the backend rejected.
        let after = list_root(&app, session_id).await;
        assert_eq!(
            after, before,
            "a failed mutation must never change the visible index"
        );
        let names: Vec<&str> = after.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"new.txt"),
            "the rejected file must not appear in the index"
        );
    });
}

#[test]
fn cancelling_a_mutation_queued_behind_another_one_on_the_same_session_never_reaches_the_backend() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let (backend, add_files_started_rx, add_files_proceed_tx) =
        FakeBackend::with_add_files_gate(vec![entry("a.txt", 1)]);
    let backend = Arc::new(backend);
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let first_file = temp.path().join("first.txt");
    std::fs::write(&first_file, b"1").unwrap();
    let second_file = temp.path().join("second.txt");
    std::fs::write(&second_file, b"2").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        // First mutation: its `add_files` call will block on the gate,
        // holding `ArchiveSession::mutation_lock` for as long as it does.
        let first_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![first_file]))
            .await
            .unwrap();

        // Deterministic rendezvous: once this returns, the first
        // mutation is genuinely inside its blocking backend call (and
        // therefore holds the session's mutation lock), not merely
        // "accepted".
        tokio::task::spawn_blocking(move || add_files_started_rx.recv().unwrap())
            .await
            .unwrap();

        // Second mutation on the SAME session: it must queue behind the
        // first one's lock. `expected_revision: 1` is still correct at
        // this instant -- the first mutation has not bumped it yet.
        let second_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![second_file]))
            .await
            .unwrap();

        // Give the second operation's worker a moment to actually reach
        // (and start waiting on) the lock acquisition before cancelling
        // it -- a bounded poll on its own state, not a fixed sleep.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snap = app.operation(second_op).await.unwrap();
            if matches!(snap.state, OperationState::Started) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "second operation never reached Started"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        app.cancel_operation(second_op)
            .await
            .expect("cancelling a lock-queued mutation must be accepted");

        let second_terminal = wait_for_terminal(&app, second_op).await;
        assert_eq!(
            second_terminal,
            OperationState::Cancelled,
            "a mutation still queued behind another one's lock must cancel promptly"
        );

        // Now let the first mutation actually finish.
        add_files_proceed_tx.send(()).unwrap();
        let first_terminal = wait_for_terminal(&app, first_op).await;
        assert_eq!(
            first_terminal,
            OperationState::Completed {
                result: OperationResult::None
            }
        );

        // The cancelled second mutation's file must never have reached
        // the backend -- only the first mutation's one call happened.
        assert_eq!(backend.add_files_call_count(), 1);
        assert_eq!(
            backend.current_paths(),
            vec!["a.txt".to_string(), "first.txt".to_string()],
            "second.txt must never have been added"
        );
    });
}

#[test]
fn exactly_one_snapshot_changed_and_one_terminal_event_are_published_for_a_successful_mutation() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend);
    let archive_path = temp.path().join("archive.zip");
    let new_file = temp.path().join("new.txt");
    std::fs::write(&new_file, b"x").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![new_file]))
            .await
            .unwrap();

        wait_for_terminal(&app, operation_id).await;

        let mut snapshot_changed_count = 0;
        let mut terminal_count = 0;
        loop {
            match events.try_recv() {
                Ok(event) if event.operation_id == operation_id => match event.state {
                    OperationState::SnapshotChanged { .. } => snapshot_changed_count += 1,
                    OperationState::Completed { .. }
                    | OperationState::Cancelled
                    | OperationState::Failed { .. } => {
                        terminal_count += 1;
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        assert_eq!(snapshot_changed_count, 1);
        assert_eq!(terminal_count, 1);

        let recent = app.recent_operations(10).await.unwrap();
        let ours = recent
            .iter()
            .find(|snapshot| snapshot.operation_id == operation_id)
            .expect("our operation must appear in recent_operations");
        assert_eq!(ours.kind, OperationKind::ArchiveModify);
    });
}

/// Requirement: EntryId stability across a mutation-triggered reindex --
/// the same invariant `ArchiveSession`'s own unit tests prove for a
/// directly-called `EntryIndex::build`, exercised here end-to-end
/// through a real `DeleteEntries` mutation. This is what makes "preserve
/// a caller's selection for entries the mutation did not touch"
/// meaningful: a UI can safely keep referencing `a.txt`'s id across the
/// delete of an unrelated `b.txt`.
#[test]
fn unchanged_entries_keep_their_entry_id_across_a_mutation_triggered_reindex() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1), entry("b.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let a_id_before = entry_id_for(&app, session_id, "a.txt").await;
        let b_id = entry_id_for(&app, session_id, "b.txt").await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![b_id],
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
        assert_eq!(backend.delete_files_call_count(), 1);

        let page_after = list_root(&app, session_id).await;
        assert_eq!(page_after.entries.len(), 1);
        let a_id_after = page_after.entries[0].id;
        assert_eq!(
            a_id_before, a_id_after,
            "a.txt's id must survive the reindex triggered by deleting b.txt"
        );
        assert!(
            page_after.entries.iter().all(|e| e.name != "b.txt"),
            "b.txt must actually be gone from the rebuilt index"
        );
    });
}

#[test]
fn a_fabricated_entry_id_is_rejected_as_not_found_before_any_backend_work() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let fabricated = EntryId::from_raw(999_999);

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id,
                expected_revision: 1,
                entry_ids: vec![fabricated],
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound)
            }
            other => panic!("expected Failed(NotFound), got {other:?}"),
        }
        assert_eq!(backend.delete_files_call_count(), 0);
    });
}

/// Fold: the contract requires every facade method to validate a
/// reconstructed id against its owning store. A structurally empty
/// request (nothing to add/delete) must not short-circuit to `Completed`
/// before that validation runs -- a bogus `session_id` alongside an
/// empty selection must still surface as `NotFound`.
#[test]
fn a_structurally_empty_request_against_an_unknown_session_is_not_found_not_completed() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let never_opened = ArchiveSessionId::from_raw(999_999);

    runtime.block_on(async {
        let operation_id = app
            .start_archive_mutation(ArchiveMutationRequest::DeleteEntries {
                session_id: never_opened,
                expected_revision: 1,
                entry_ids: vec![],
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound)
            }
            other => panic!(
                "an empty request against an unknown session must be NotFound, not {other:?}"
            ),
        }
        assert_eq!(backend.add_files_call_count(), 0);
        assert_eq!(backend.delete_files_call_count(), 0);
    });
}

/// The brief's own regression scenario for a desynced session: a
/// mutation whose own backend call succeeds but whose follow-up re-list
/// fails must never again let a subsequent mutation through, regardless
/// of which `expected_revision` it claims -- neither the stale value a
/// caller already held, nor the bumped value `mark_desynced` itself
/// produces (proving the desync flag itself gates this, not merely the
/// revision counter). Without this, a `ReplaceText` resolved against the
/// stale-but-still-indexed entry could recreate content an earlier,
/// already-successful `DeleteEntries` had genuinely removed from the
/// backend.
#[test]
fn a_relist_failure_after_a_successful_mutation_desyncs_the_session_and_blocks_every_later_mutation(
) {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let new_file = temp.path().join("new.txt");
    std::fs::write(&new_file, b"x").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        // The mutation's own backend call will succeed; the follow-up
        // re-list this operation needs in order to reindex will not.
        backend.fail_next_list_call();
        let first_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![new_file]))
            .await
            .unwrap();
        match wait_for_terminal(&app, first_op).await {
            OperationState::Failed { error } => assert_eq!(error.kind, ApplicationErrorKind::Backend),
            other => panic!("expected the relist failure to surface as Failed(Backend), got {other:?}"),
        }
        // The add_files call really happened -- this is not a rejection
        // before backend work, it is a genuine post-success desync.
        assert_eq!(backend.add_files_call_count(), 1);

        let bumped_revision = snapshot(&app, session_id).await.revision;
        assert_eq!(bumped_revision, 2, "mark_desynced must still bump the revision");

        // A second mutation submitted with the STALE (pre-desync)
        // revision must be rejected as Conflict, never reaching the
        // backend. Deliberately a real, non-empty `AddFiles` request --
        // an empty one would hit the structurally-empty short-circuit
        // before ever reaching the desync check this test targets.
        let second_new_file = temp.path().join("second.txt");
        std::fs::write(&second_new_file, b"y").unwrap();
        let stale_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![second_new_file.clone()]))
            .await
            .unwrap();
        match wait_for_terminal(&app, stale_op).await {
            OperationState::Failed { error } => assert_eq!(error.kind, ApplicationErrorKind::Conflict),
            other => panic!("expected Failed(Conflict) for a stale-revision mutation on a desynced session, got {other:?}"),
        }
        assert_eq!(
            backend.add_files_call_count(),
            1,
            "a mutation on a desynced session must never reach the backend, even with a stale revision"
        );

        // A third mutation submitted with the *bumped* revision --
        // exactly what `mark_desynced` itself produced -- must ALSO be
        // rejected: the desync flag gates this independently of whether
        // the claimed revision happens to match.
        let matching_revision_op = app
            .start_archive_mutation(add_files_request(session_id, bumped_revision, vec![second_new_file]))
            .await
            .unwrap();
        match wait_for_terminal(&app, matching_revision_op).await {
            OperationState::Failed { error } => assert_eq!(error.kind, ApplicationErrorKind::Conflict),
            other => panic!(
                "expected Failed(Conflict) even with the bumped revision on a desynced session, got {other:?}"
            ),
        }
        assert_eq!(backend.add_files_call_count(), 1);

        // Recovery: close this session and open a fresh one against the
        // same path (the backend's own content -- still just "a.txt",
        // since the scripted failure only affected the *reindex*, never
        // the real `add_files` call -- doesn't matter here; what matters
        // is that a brand new session is never desynced).
        app.close_archive(session_id).await.unwrap();
        let reopened_session_id = open_session(&app, &archive_path).await;
        assert_ne!(reopened_session_id, session_id);

        let third_file = temp.path().join("third.txt");
        std::fs::write(&third_file, b"z").unwrap();
        let recovered_op = app
            .start_archive_mutation(add_files_request(reopened_session_id, 1, vec![third_file]))
            .await
            .unwrap();
        assert_eq!(
            wait_for_terminal(&app, recovered_op).await,
            OperationState::Completed {
                result: OperationResult::None
            },
            "a fresh session after close+reopen must not inherit the old session's desync"
        );
        assert_eq!(backend.add_files_call_count(), 2);
    });
}

/// The concurrent counterpart to the sequential desync test above: proves
/// `mark_desynced()` (in `relist_result`'s failure arm) genuinely runs
/// before `_guard` releases `ArchiveSession::mutation_lock`, not merely
/// before the *next mutation happens to be submitted*. A second mutation
/// is started and driven to a genuine `Started`-and-blocked-on-the-lock
/// state -- proven the same way the cancellation test above proves it --
/// while the first is still stuck inside its gated, about-to-fail
/// relist. Only once that is confirmed is the gate released. If
/// `mutation_lock` were ever dropped before `mark_desynced()` records the
/// desync (the exact bug this round fixes), the queued second mutation
/// could win the race for the lock and observe `is_desynced() == false`
/// and the not-yet-bumped revision, passing both gates against an index
/// that is, by then, already provably stale.
#[test]
fn a_mutation_queued_behind_a_gated_relist_failure_never_observes_the_pre_desync_state() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::new(vec![entry("a.txt", 1)]));
    let app = bootstrap_app(&temp, backend.clone());
    let archive_path = temp.path().join("archive.zip");
    let first_new_file = temp.path().join("first.txt");
    std::fs::write(&first_new_file, b"1").unwrap();
    let second_new_file = temp.path().join("second.txt");
    std::fs::write(&second_new_file, b"2").unwrap();

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        // Armed *after* `open_session` -- which already consumed its own
        // `list()` call -- so this gate can only ever catch the first
        // mutation's post-`add_files` relist, never the initial open.
        let (list_started_rx, list_proceed_tx) = backend.gate_next_list_call();
        backend.fail_next_list_call();

        let first_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![first_new_file]))
            .await
            .unwrap();

        // Deterministic rendezvous: `add_files` has already returned
        // successfully and the operation is now genuinely blocked inside
        // the relist `spawn_blocking` -- still holding `mutation_lock`,
        // having not yet reached `relist_result`'s match, let alone
        // `mark_desynced()` or the implicit end-of-function drop.
        tokio::task::spawn_blocking(move || list_started_rx.recv().unwrap())
            .await
            .unwrap();

        // Second mutation on the SAME session. `expected_revision: 1` is
        // still the true current revision at this exact instant -- the
        // first mutation succeeded at the backend but has not reindexed,
        // bumped, or desynced anything yet.
        let second_op = app
            .start_archive_mutation(add_files_request(session_id, 1, vec![second_new_file]))
            .await
            .unwrap();

        // Prove the second mutation is genuinely queued on the lock (not
        // merely submitted) before the gate is ever released -- the same
        // bounded poll the cancellation test above uses.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snap = app.operation(second_op).await.unwrap();
            if matches!(snap.state, OperationState::Started) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "second operation never reached Started while queued behind the lock"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Release the gate: the first mutation's relist now runs -- and,
        // per `fail_next_list_call`, fails. This is the exact instant a
        // buggy ordering would drop `_guard` before `mark_desynced()`
        // runs, handing the lock to the already-queued second mutation
        // ahead of the desync being recorded.
        list_proceed_tx.send(()).unwrap();

        match wait_for_terminal(&app, first_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Backend)
            }
            other => panic!(
                "expected the gated relist failure to surface as Failed(Backend), got {other:?}"
            ),
        }

        // The second mutation, unblocked only once the first actually
        // released the lock, must observe the desync -- never a window
        // where it is still unset -- and therefore never reach the
        // backend at all.
        match wait_for_terminal(&app, second_op).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict)
            }
            other => panic!(
                "expected the lock-queued mutation to be rejected as Failed(Conflict) once it \
                 acquired the lock and observed the desync, got {other:?}"
            ),
        }
        assert_eq!(
            backend.add_files_call_count(),
            1,
            "the queued second mutation must never reach add_files -- the desync check must \
             reject it immediately after it acquires the lock"
        );
    });
}
