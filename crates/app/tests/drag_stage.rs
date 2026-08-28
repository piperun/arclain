//! Integration tests for the drag-stage surface: `start_drag_stage`'s
//! multi-entry staging lifecycle, the synchronous
//! `stage_drag_payload_blocking` affordance an OS drag source calls from
//! its own foreign thread, and the self-renewing `DragStagingLease` it
//! returns.
//!
//! Fake-backend shape mirrors `materialization_leases.rs`: every test
//! installs an `ArchiveBackend` via
//! `BootstrapConfig::archive_backend_override` that lists a fixed entry
//! set and captures/serves extraction. Fixture names use anonymized
//! RJ123456-style placeholders.
//!
//! Every test is a plain (synchronous) `#[test]` for the same reason
//! `materialization_leases.rs`'s are: dropping `ArclainApp` must not
//! happen from inside an async context -- and here the synchronous shape
//! is doubly load-bearing, because `stage_drag_payload_blocking`'s whole
//! contract is "called from a thread that is not a runtime worker",
//! which the test's own main thread genuinely is.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::EntryId;
use arclain_app::materialization::{DragStageEvent, DragStageRequest};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn bootstrap_app(paths: AppPaths, backend: Arc<dyn arclain_core::ArchiveBackend>) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        archive_backend_override: Some(backend),
        ..Default::default()
    })
    .expect("bootstrap must succeed")
}

/// Short lease TTL + fast sweep so the self-renewal test can prove the
/// lease outlives its own unrenewed deadline only because the
/// `DragStagingLease` keeps renewing it.
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

fn dir(path: &str) -> arclain_core::ArchiveEntry {
    arclain_core::ArchiveEntry {
        path: path.to_string(),
        size: 0,
        packed_size: 0,
        modified: None,
        is_dir: true,
        encrypted: false,
        crc32: None,
    }
}

/// Which extraction entry point the worker actually invoked -- the batch
/// tests assert strategy selection, not just resulting bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ExtractCall {
    FilesWithProgress(Vec<String>),
    Directory(String),
    All,
}

/// A fake `ArchiveBackend` that lists a fixed entry set, records every
/// extraction call, and serves extraction by writing `content_for` bytes
/// (default: the entry path itself) for each known file under whatever
/// destination it is given. `fail_with` scripts a failure instead.
struct RecordingBackend {
    entries: Vec<arclain_core::ArchiveEntry>,
    contents: std::collections::HashMap<String, Vec<u8>>,
    calls: Mutex<Vec<ExtractCall>>,
    fail_with: Option<String>,
}

impl RecordingBackend {
    fn new(entries: Vec<arclain_core::ArchiveEntry>, contents: &[(&str, &[u8])]) -> Arc<Self> {
        Arc::new(Self {
            entries,
            contents: contents
                .iter()
                .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
                .collect(),
            calls: Mutex::new(Vec::new()),
            fail_with: None,
        })
    }

    fn failing(entries: Vec<arclain_core::ArchiveEntry>, message: &str) -> Arc<Self> {
        Arc::new(Self {
            entries,
            contents: std::collections::HashMap::new(),
            calls: Mutex::new(Vec::new()),
            fail_with: Some(message.to_string()),
        })
    }

    fn calls(&self) -> Vec<ExtractCall> {
        self.calls.lock().unwrap().clone()
    }

    fn write_files<'a>(&self, dest: &Path, files: impl Iterator<Item = &'a String>) {
        for path in files {
            let content = self
                .contents
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.as_bytes().to_vec());
            let target = dest.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(target, content).unwrap();
        }
    }
}

impl arclain_core::ArchiveBackend for RecordingBackend {
    fn name(&self) -> &str {
        "recording"
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
        dest: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(ExtractCall::All);
        if let Some(message) = &self.fail_with {
            anyhow::bail!("{message}");
        }
        let all: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        self.write_files(dest, all.iter());
        Ok(())
    }
    fn extract_files(
        &self,
        _path: &Path,
        _dest: &Path,
        _files: &[String],
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!("the drag-stage worker always calls extract_files_with_progress")
    }
    fn extract_files_with_progress(
        &self,
        _path: &Path,
        dest: &Path,
        files: &[String],
        _password: Option<&str>,
        _progress: Option<&arclain_core::ProgressCallback>,
        _cancel: Option<&arclain_core::CancellationToken>,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(ExtractCall::FilesWithProgress(files.to_vec()));
        if let Some(message) = &self.fail_with {
            anyhow::bail!("{message}");
        }
        self.write_files(dest, files.iter());
        Ok(())
    }
    fn extract_directory(
        &self,
        _path: &Path,
        dest: &Path,
        dir_path: &str,
        _password: Option<&str>,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(ExtractCall::Directory(dir_path.to_string()));
        if let Some(message) = &self.fail_with {
            anyhow::bail!("{message}");
        }
        let prefix = format!("{}/", dir_path.trim_end_matches('/'));
        let under: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.is_dir && (dir_path.is_empty() || e.path.starts_with(&prefix)))
            .map(|e| e.path.clone())
            .collect();
        self.write_files(dest, under.iter());
        Ok(())
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

/// A fake backend whose extraction rendezvouses on a barrier (so a test
/// deterministically knows the worker is mid-extraction) and then loops
/// on the cancellation token -- the same shape as
/// `materialization_leases.rs`'s `BarrierBackend`.
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
                if cancel.load(Ordering::SeqCst) {
                    anyhow::bail!("extraction cancelled");
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn extract_directory(
        &self,
        _p: &Path,
        _d: &Path,
        _dir: &str,
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

async fn open_session(app: &ArclainApp, path: &Path) -> arclain_app::ids::ArchiveSessionId {
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

async fn wait_for_terminal(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> OperationState {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        if matches!(
            snapshot.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return snapshot.state;
        }
        if std::time::Instant::now() >= deadline {
            panic!("drag stage did not reach a terminal state within the test deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn expect_staged(state: OperationState) -> arclain_app::materialization::MaterializationLease {
    match state {
        OperationState::Completed {
            result: OperationResult::Materialized { lease },
        } => lease,
        OperationState::Failed { error } => panic!("drag stage unexpectedly failed: {error:?}"),
        other => panic!("expected Completed(Materialized), got {other:?}"),
    }
}

fn poll_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if probe() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ============================== Tests ==============================

#[test]
fn staging_a_mixed_multi_entry_selection_extracts_exactly_the_selection() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = RecordingBackend::new(
        vec![
            file("RJ123456/scene_a.dat"),
            file("RJ123456/img/cover.png"),
            file("readme.txt"),
            file("unrelated/other.bin"),
        ],
        &[
            ("RJ123456/scene_a.dat", b"scene-a-bytes"),
            ("RJ123456/img/cover.png", b"cover-bytes"),
            ("readme.txt", b"readme-bytes"),
            ("unrelated/other.bin", b"must-not-be-staged"),
        ],
    );
    let app = bootstrap_app(paths, backend.clone());
    let archive_path = temp.path().join("RJ123456.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let dir_id = entry_id_for(&app, session_id, "", "RJ123456").await;
        let readme_id = entry_id_for(&app, session_id, "", "readme.txt").await;

        let operation_id = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: vec![dir_id, readme_id],
            })
            .await
            .expect("start_drag_stage must be accepted");

        let lease = expect_staged(wait_for_terminal(&app, operation_id).await);

        // The lease's local_path is the staging ROOT, and it contains
        // exactly the selection: the directory's whole subtree plus the
        // separately selected root file -- byte-identical.
        assert!(lease.local_path.is_dir());
        assert_eq!(
            std::fs::read(lease.local_path.join("RJ123456/scene_a.dat")).unwrap(),
            b"scene-a-bytes"
        );
        assert_eq!(
            std::fs::read(lease.local_path.join("RJ123456/img/cover.png")).unwrap(),
            b"cover-bytes"
        );
        assert_eq!(
            std::fs::read(lease.local_path.join("readme.txt")).unwrap(),
            b"readme-bytes"
        );
        assert!(
            !lease.local_path.join("unrelated").exists(),
            "an unselected sibling must not be staged on the direct-extract path"
        );

        // Strategy: a small selection goes through the per-file call with
        // exactly the expanded selection, nothing more.
        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            ExtractCall::FilesWithProgress(files) => {
                let mut files = files.clone();
                files.sort();
                assert_eq!(
                    files,
                    [
                        "RJ123456/img/cover.png",
                        "RJ123456/scene_a.dat",
                        "readme.txt"
                    ]
                );
            }
            other => panic!("expected FilesWithProgress, got {other:?}"),
        }

        // The operation is its own kind, visible as such in history.
        let recent = app.recent_operations(10).await.unwrap();
        let ours = recent
            .iter()
            .find(|snapshot| snapshot.operation_id == operation_id)
            .expect("the drag stage must appear in recent_operations");
        assert_eq!(ours.kind, OperationKind::DragStage);
    });
}

#[test]
fn a_selection_past_the_direct_extract_cap_switches_to_a_batch_strategy() {
    // The 7-Zip CLI backend silently truncates over-long command lines;
    // the worker must therefore never pass a >75-file list to the
    // per-file entry point (see `drag_stage`'s module doc comment).
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let mut entries = Vec::new();
    for i in 0..80 {
        entries.push(file(&format!("RJ123456/part_{i:03}.dat")));
    }
    let backend = RecordingBackend::new(entries, &[]);
    let app = bootstrap_app(paths, backend.clone());
    let archive_path = temp.path().join("RJ123456.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let dir_id = entry_id_for(&app, session_id, "", "RJ123456").await;

        let operation_id = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: vec![dir_id],
            })
            .await
            .unwrap();

        let lease = expect_staged(wait_for_terminal(&app, operation_id).await);

        assert_eq!(
            backend.calls(),
            vec![ExtractCall::Directory("RJ123456".to_string())],
            "80 resolved files must extract via the common-directory batch, never the \
             truncation-prone per-file call"
        );
        assert!(lease.local_path.join("RJ123456/part_000.dat").exists());
        assert!(lease.local_path.join("RJ123456/part_079.dat").exists());
    });
}

#[test]
fn an_empty_selection_is_rejected_and_an_unknown_entry_id_is_not_found() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = RecordingBackend::new(vec![file("a.txt")], &[]);
    let app = bootstrap_app(paths, backend.clone());
    let archive_path = temp.path().join("RJ123456.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;

        let empty = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: Vec::new(),
            })
            .await
            .unwrap();
        match wait_for_terminal(&app, empty).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
            }
            other => panic!("expected Failed(InvalidInput), got {other:?}"),
        }

        let unknown = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: vec![EntryId::from_raw(999_999)],
            })
            .await
            .unwrap();
        match wait_for_terminal(&app, unknown).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound);
            }
            other => panic!("expected Failed(NotFound), got {other:?}"),
        }

        assert!(
            backend.calls().is_empty(),
            "a rejected request must never reach the backend"
        );
    });
}

#[test]
fn a_selected_empty_directory_is_staged_as_an_empty_directory_without_touching_the_backend() {
    // Zero resolved files must never reach the backend: the real 7-Zip
    // CLI treats an empty file-argument list as "no filter" and would
    // extract the ENTIRE archive into the staging directory.
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = RecordingBackend::new(vec![dir("empty_dir"), file("huge/blob.bin")], &[]);
    let app = bootstrap_app(paths, backend.clone());
    let archive_path = temp.path().join("RJ123456.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let dir_id = entry_id_for(&app, session_id, "", "empty_dir").await;

        let operation_id = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: vec![dir_id],
            })
            .await
            .unwrap();

        let lease = expect_staged(wait_for_terminal(&app, operation_id).await);

        assert!(lease.local_path.join("empty_dir").is_dir());
        assert!(
            backend.calls().is_empty(),
            "an all-directories selection with no files must not invoke the backend at all"
        );
        assert!(
            !lease.local_path.join("huge").exists(),
            "nothing else may be extracted as a side effect"
        );
    });
}

#[test]
fn a_password_shaped_failure_fails_fast_without_ever_raising_a_challenge() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let materialization_dir = paths.cache_dir.join("materialization");
    let backend = RecordingBackend::failing(vec![file("secret.txt")], "Wrong password for archive");
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("RJ123456.zip");

    runtime.block_on(async {
        let session_id = open_session(&app, &archive_path).await;
        let entry_id = entry_id_for(&app, session_id, "", "secret.txt").await;
        let mut events = app.subscribe_operations();

        let operation_id = app
            .start_drag_stage(DragStageRequest {
                session_id,
                entry_ids: vec![entry_id],
            })
            .await
            .unwrap();

        match wait_for_terminal(&app, operation_id).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::PasswordRequired);
            }
            other => panic!("expected Failed(PasswordRequired), got {other:?}"),
        }

        // Drain everything the operation ever published: a Challenge would
        // freeze the OS shell mid-drop, so none may ever have been raised.
        loop {
            match events.try_recv() {
                Ok(event) if event.operation_id == operation_id => {
                    assert!(
                        !matches!(event.state, OperationState::Challenge { .. }),
                        "a drag stage must fail fast on password errors, never challenge"
                    );
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // The failed stage's reserved directory must have been cleaned up.
    let leftover = std::fs::read_dir(&materialization_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        leftover, 0,
        "a failed drag stage must not leave its staging directory behind"
    );
}

#[test]
fn blocking_stage_from_a_foreign_thread_returns_a_self_renewing_lease_and_reports_events() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let materialization_dir = paths.cache_dir.join("materialization");
    let backend = RecordingBackend::new(
        vec![file("RJ123456/scene_a.dat")],
        &[("RJ123456/scene_a.dat", b"drag-staged-bytes")],
    );
    // 150ms TTL: without renewal the lease would be swept long before the
    // 600ms probe below -- surviving it proves DragStagingLease renews.
    let app = bootstrap_app_with_short_lease_lifetime(paths, backend);
    let archive_path = temp.path().join("RJ123456.zip");

    // Session setup awaited through a scoped foreign runtime; the
    // blocking call itself runs on this plain test thread afterwards,
    // exactly the "foreign OS thread" shape the affordance exists for.
    let (session_id, entry_id) = {
        let runtime = foreign_runtime();
        runtime.block_on(async {
            let session_id = open_session(&app, &archive_path).await;
            let entry_id = entry_id_for(&app, session_id, "", "RJ123456").await;
            (session_id, entry_id)
        })
    };

    let mut events: Vec<DragStageEvent> = Vec::new();
    let staged = app
        .stage_drag_payload_blocking(
            DragStageRequest {
                session_id,
                entry_ids: vec![entry_id],
            },
            &mut |event| events.push(event),
        )
        .expect("blocking stage from a genuinely non-runtime thread must succeed");

    // Events: Started first (with the cancellable operation id), then at
    // least one Progress tick.
    assert!(
        matches!(events.first(), Some(DragStageEvent::Started { .. })),
        "the first observed event must be Started, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DragStageEvent::Progress { .. })),
        "at least one Progress event must reach the observer, got {events:?}"
    );

    let staged_file = staged.local_root().join("RJ123456/scene_a.dat");
    assert_eq!(std::fs::read(&staged_file).unwrap(), b"drag-staged-bytes");

    // Outlive the unrenewed TTL by 4x: only live renewal explains survival.
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        staged_file.exists(),
        "the staged payload must survive past its unrenewed TTL while the lease handle lives"
    );

    // Dropping the handle releases the lease and removes the directory.
    drop(staged);
    assert!(
        poll_until(Duration::from_secs(5), || !std::fs::read_dir(
            &materialization_dir
        )
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)),
        "dropping the DragStagingLease must release the lease and remove its directory"
    );
}

#[test]
fn cancelling_by_operation_id_unblocks_a_blocked_stage_and_leaks_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let materialization_dir = paths.cache_dir.join("materialization");
    let start_barrier = Arc::new(std::sync::Barrier::new(2));
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(BarrierBackend {
        entries: vec![file("a.txt")],
        start_barrier: start_barrier.clone(),
    });
    let app = bootstrap_app(paths, backend);
    let archive_path = temp.path().join("RJ123456.zip");

    let (session_id, entry_id) = {
        let runtime = foreign_runtime();
        runtime.block_on(async {
            let session_id = open_session(&app, &archive_path).await;
            let entry_id = entry_id_for(&app, session_id, "", "a.txt").await;
            (session_id, entry_id)
        })
    };

    let (op_tx, op_rx) = std::sync::mpsc::channel();
    let blocked = {
        let app = app.clone();
        std::thread::spawn(move || {
            app.stage_drag_payload_blocking(
                DragStageRequest {
                    session_id,
                    entry_ids: vec![entry_id],
                },
                &mut |event| {
                    if let DragStageEvent::Started { operation_id } = event {
                        let _ = op_tx.send(operation_id);
                    }
                },
            )
        })
    };

    let operation_id = op_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the blocked thread must report Started");
    // Rendezvous: the worker is provably inside the backend call now.
    start_barrier.wait();

    {
        let runtime = foreign_runtime();
        runtime.block_on(async {
            app.cancel_operation(operation_id)
                .await
                .expect("cancelling a running drag stage must be accepted");
        });
    }

    let result = blocked
        .join()
        .expect("the blocked thread must not panic on cancellation");
    match result {
        Err(error) => assert_eq!(error.kind, ApplicationErrorKind::Cancelled),
        Ok(_) => panic!("a cancelled drag stage must not return a lease"),
    }

    // The worker finishes cooperatively (the barrier backend notices the
    // token within ~5ms) and its reservation guard removes the staging
    // directory -- bounded poll, no leak.
    assert!(
        poll_until(Duration::from_secs(5), || std::fs::read_dir(
            &materialization_dir
        )
        .map(|entries| entries.count() == 0)
        .unwrap_or(true)),
        "a cancelled drag stage must not leave its staging directory behind"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "must never be called from inside a Tokio runtime")]
fn blocking_stage_from_inside_a_runtime_panics_the_debug_assertion() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let backend = RecordingBackend::new(vec![file("a.txt")], &[]);
    let app = bootstrap_app(paths, backend);

    let runtime = foreign_runtime();
    // `block_on` enters the runtime context, so `Handle::try_current()`
    // resolves inside it -- exactly the misuse the assertion pins.
    let _ = runtime.block_on(async {
        app.stage_drag_payload_blocking(
            DragStageRequest {
                session_id: arclain_app::ids::ArchiveSessionId::from_raw(1),
                entry_ids: vec![EntryId::from_raw(1)],
            },
            &mut |_| {},
        )
    });
}

#[test]
fn the_new_operation_kind_serializes_with_the_contract_naming_scheme() {
    // A drag source hands the staged lease across threads (the COM data
    // object lives on an STA thread; release fires from wherever the
    // last COM reference drops) -- pin `Send` at compile time.
    fn assert_send<T: Send>() {}
    assert_send::<arclain_app::materialization::DragStagingLease>();

    assert_eq!(
        serde_json::to_string(&OperationKind::DragStage).unwrap(),
        "\"drag_stage\""
    );
    assert_eq!(
        serde_json::from_str::<OperationKind>("\"drag_stage\"").unwrap(),
        OperationKind::DragStage
    );
}
