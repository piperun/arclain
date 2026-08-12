//! End-to-end coverage for the facade-backed drag payload source: the
//! production seam `crate::platform::drag_source::windows` stages
//! through when the shell commits to a drop.
//!
//! The happy path runs against a REAL bootstrapped `ArclainApp` and a
//! real on-disk ZIP fixture (same precondition as
//! `archive_mutation_ui_test.rs`: a real 7-Zip on `PATH` for the
//! facade's default backend selection), because its whole point is
//! byte-identity between the archive's content and what a completed
//! drag hands the shell. Deterministic backend cancellation coverage
//! lives in `crates/app/tests/drag_stage.rs`, next to the application
//! seam that owns it.
//!
//! Fixture names use anonymized RJ123456-style placeholders.

use std::path::Path;
use std::time::Duration;

use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::EntryId;
use arclain_ui::platform::drag_source::{
    DragPayloadSource, DragProgressUpdate, FacadeDragPayloadSource,
};

fn temp_paths(root: &Path) -> arclain_app::AppPaths {
    arclain_app::AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
    }
}

fn bootstrap_real_app(temp: &tempfile::TempDir) -> arclain_app::ArclainApp {
    arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(temp_paths(temp.path())),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("bootstrap must succeed against a bare temp-dir AppPaths")
}

fn build_zip_fixture(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in entries {
        writer
            .start_file(*name, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry");
    }
    writer.finish().expect("finish zip fixture");
}

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A multi-thread runtime whose handle can be given to
/// `FacadeDragPayloadSource` for its fire-and-forget `request_cancel`
/// spawns: unlike [`foreign_runtime`]'s current-thread flavor, its
/// worker threads poll spawned tasks without anyone calling `block_on`
/// -- which is exactly what a fire-and-forget spawn needs. (Production
/// passes the application's own multi-thread runtime handle.)
fn spawn_capable_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

async fn open_session(
    app: &arclain_app::ArclainApp,
    archive: &Path,
) -> arclain_app::ids::ArchiveSessionId {
    let operation_id = app
        .start_open_archive(arclain_app::archive::OpenArchiveRequest {
            source_path: archive.to_path_buf(),
            password: None,
        })
        .await
        .expect("start_open_archive must be accepted");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
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
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            _ => panic!("archive open did not complete within the test deadline"),
        }
    }
}

async fn entry_id_for(
    app: &arclain_app::ArclainApp,
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

async fn drag_stage_operation_count(app: &arclain_app::ArclainApp) -> usize {
    app.recent_operations(50)
        .await
        .expect("recent_operations must succeed")
        .iter()
        .filter(|snapshot| snapshot.kind == OperationKind::DragStage)
        .count()
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
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn materialization_dir_is_empty(paths: &arclain_app::AppPaths) -> bool {
    std::fs::read_dir(paths.cache_dir.join("materialization"))
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(true)
}

/// The full production drop path against a real archive: construct the
/// payload source (proving construction alone starts nothing -- the
/// hover-time contract), stage from a genuinely non-runtime thread
/// (this test's own), verify byte-identity against the fixture, then
/// drop the payload and verify the staged lease directory is released.
#[test]
fn staging_through_the_facade_source_is_byte_identical_and_releases_on_drop() {
    const SCENE_BYTES: &[u8] = b"scene-a: not a real scene, only its bytes matter";
    const COVER_BYTES: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x00, 0x01, 0x02, 0x03];
    const README_BYTES: &[u8] = b"hello from the drag-stage facade test";

    let temp = tempfile::tempdir().unwrap();
    let paths = temp_paths(temp.path());
    let app = bootstrap_real_app(&temp);
    let archive_path = temp.path().join("RJ123456.zip");
    build_zip_fixture(
        &archive_path,
        &[
            ("RJ123456/scene_a.dat", SCENE_BYTES),
            ("RJ123456/img/cover.png", COVER_BYTES),
            ("readme.txt", README_BYTES),
        ],
    );

    // Session setup through a scoped foreign runtime; the staging call
    // itself happens on this plain test thread afterwards.
    let cancel_runtime = spawn_capable_runtime();
    let (session_id, dir_id, readme_id) = {
        let runtime = foreign_runtime();
        runtime.block_on(async {
            let session_id = open_session(&app, &archive_path).await;
            let dir_id = entry_id_for(&app, session_id, "", "RJ123456").await;
            let readme_id = entry_id_for(&app, session_id, "", "readme.txt").await;
            (session_id, dir_id, readme_id)
        })
    };

    let source = FacadeDragPayloadSource::new(
        app.clone(),
        cancel_runtime.handle().clone(),
        session_id,
        vec![dir_id, readme_id],
    );

    // Hover contract: building the source (what drag start does) must
    // not have started any staging operation -- staging is drop-time
    // only.
    {
        let runtime = foreign_runtime();
        let count = runtime.block_on(drag_stage_operation_count(&app));
        assert_eq!(
            count, 0,
            "constructing the drag payload source must not start a staging operation"
        );
    }

    let mut progress: Vec<DragProgressUpdate> = Vec::new();
    let staged = source
        .stage_blocking(&mut |update| progress.push(update))
        .expect("staging a real selection through the facade must succeed");

    // Byte-identity: the staged tree matches the archive's own content,
    // shaped by the selection (directory subtree + separate root file).
    assert_eq!(
        std::fs::read(staged.root().join("RJ123456/scene_a.dat")).unwrap(),
        SCENE_BYTES
    );
    assert_eq!(
        std::fs::read(staged.root().join("RJ123456/img/cover.png")).unwrap(),
        COVER_BYTES
    );
    assert_eq!(
        std::fs::read(staged.root().join("readme.txt")).unwrap(),
        README_BYTES
    );

    assert!(
        !progress.is_empty(),
        "staging progress must reach the drag progress callback"
    );

    // Exactly one DragStage operation ran, and it completed.
    {
        let runtime = foreign_runtime();
        runtime.block_on(async {
            let recent = app.recent_operations(50).await.unwrap();
            let ours: Vec<_> = recent
                .iter()
                .filter(|snapshot| snapshot.kind == OperationKind::DragStage)
                .collect();
            assert_eq!(ours.len(), 1);
            assert!(matches!(ours[0].state, OperationState::Completed { .. }));
        });
    }

    // Releasing: dropping the staged payload (what the COM object does
    // when the shell releases it) must release the lease directory.
    let staged_root = staged.root().to_path_buf();
    assert!(staged_root.exists());
    drop(staged);
    assert!(
        poll_until(Duration::from_secs(10), || materialization_dir_is_empty(
            &paths
        )),
        "dropping the staged payload must release its materialization lease directory"
    );
    assert!(!staged_root.exists());
}
