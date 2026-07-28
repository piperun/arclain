//! End-to-end coverage for the UI-side archive-mutation wiring added in
//! this task: `crate::core::operations::file::start_add_files`/
//! `start_replace_text` and `crate::features::archive_browser::
//! application::FileOpsService::delete_files`, driven through a real
//! bootstrapped `ArclainApp` the same way
//! `archive_session_lifecycle_test.rs` drives open/close.
//!
//! Unlike `crates/app/tests/archive_mutation.rs` (which fakes the
//! backend via `BootstrapConfig::archive_backend_override` to test the
//! *operation's* own orchestration), the happy-path test here uses a
//! real, on-disk ZIP file and no backend override at all. That is a
//! deliberate consequence of a real characterization finding: this
//! crate's own archive-browser refresh
//! (`crate::core::operation_bridge::refresh_entries_after_mutation`, and
//! `relist_for_browser_signals` before it, for `OpenArchive`) resolves
//! its backend through `SharedState::app_state`'s own
//! extension-based `BackendSelector` -- entirely independent of
//! whatever backend `ArclainApp::bootstrap` was given. Faking the
//! facade's backend here would make the operation succeed while the
//! UI's own re-list still tried (and failed) to open a fake path through
//! the real native ZIP/7z-CLI chain. A real, minimal (22-byte, zero
//! entries) ZIP fixture keeps both sides consistent, at the cost of
//! depending on a real 7-Zip executable on `PATH` -- the same
//! precondition `create_test_shared_state`'s own doc comment already
//! documents for every other test that uses it, and the same one
//! `archive_session_lifecycle_test.rs`'s own bootstrap helper relies on.

mod common;
use common::create_test_shared_state;

use std::path::Path;
use std::time::Duration;

/// The End Of Central Directory record for a ZIP archive with zero
/// entries -- the smallest possible valid ZIP file. Real, hand-written
/// bytes rather than the `zip` crate (added as a dev-dependency below
/// only for [`build_zip_fixture`]'s nested-entry case, which genuinely
/// needs it): every field after the 4-byte signature is legitimately
/// zero for an empty archive.
const EMPTY_ZIP_BYTES: [u8; 22] = [
    0x50, 0x4B, 0x05, 0x06, // signature "PK\x05\x06"
    0x00, 0x00, // this disk number
    0x00, 0x00, // disk with the start of the central directory
    0x00, 0x00, // central directory records on this disk
    0x00, 0x00, // total central directory records
    0x00, 0x00, 0x00, 0x00, // size of the central directory
    0x00, 0x00, 0x00, 0x00, // offset of the central directory
    0x00, 0x00, // comment length
];

fn write_empty_zip(path: &Path) {
    std::fs::write(path, EMPTY_ZIP_BYTES).expect("write empty zip fixture");
}

/// Builds a real ZIP fixture at `path` containing `entries`
/// (archive-relative path -> content) -- unlike [`write_empty_zip`],
/// this can express a nested entry (e.g. `"subdir/nested.txt"`), which a
/// hand-written empty-archive byte literal cannot. Mirrors
/// `crates/app/tests/archive_sessions.rs::build_zip_fixture` exactly.
fn build_zip_fixture(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry content");
    }
    writer.finish().expect("finish zip fixture");
}

async fn wait_for_open_completion(
    app: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        if let arclain_app::event::OperationState::Completed {
            result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
        } = snapshot.state
        {
            return snapshot;
        }
        if let arclain_app::event::OperationState::Failed { error } = snapshot.state {
            panic!("archive open unexpectedly failed: {error:?}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "open did not complete within the test deadline"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn wait_until(message: &str, predicate: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(std::time::Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Bootstraps a real `ArclainApp` against a bare temp-dir `AppPaths`,
/// with no backend override -- see this module's own doc comment for
/// why the happy-path test needs the facade and the UI's own re-list to
/// resolve through the exact same real backend chain.
fn bootstrap_real_app(temp: &tempfile::TempDir) -> arclain_app::ArclainApp {
    let paths = arclain_app::AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
    })
    .expect("bootstrap must succeed against a bare temp-dir AppPaths")
}

#[test]
fn start_add_files_reaches_a_real_backend_and_the_bridge_refreshes_the_tabs_entries() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("archive.zip");
    write_empty_zip(&archive_path);
    let new_file = temp.path().join("new_file.txt");
    std::fs::write(&new_file, b"hello from the facade").unwrap();

    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();
    // `start_add_files` is fire-and-forget: `register_operation`'s own
    // one-shot reconciliation can easily land while the operation is
    // still `Accepted`/`Started` (it does not itself wait for the
    // mutation to finish), so *something* must still be listening for
    // the later `SnapshotChanged`/`Completed` events -- exactly what
    // `spawn` provides in production (`SharedState::new` calls it once
    // per app instance). `create_test_shared_state`'s fixture
    // deliberately does not start this, so tests that only wait for
    // completion before registering (like
    // `archive_session_lifecycle_test.rs`'s) never need it -- this one
    // does.
    arclain_ui::core::operation_bridge::spawn(&shared);

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive_path.clone(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let snapshot = wait_for_open_completion(&app, operation_id).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
        snapshot.session_id
    });

    // The real UI-side re-list (through the extension-based
    // `BackendSelector`, independent of the facade) must have already
    // populated the tab from the real (empty) archive.
    wait_until(
        "the archive open never populated the tab's entries signal",
        || tab.archive_path.get().as_deref() == Some(archive_path.as_path()),
    );
    assert_eq!(
        tab.entries.get().len(),
        0,
        "the fixture archive starts with zero entries"
    );

    arclain_ui::core::operations::file::start_add_files(
        &shared,
        tab_id,
        session_id,
        vec![new_file],
    );

    wait_until(
        "start_add_files never reached a real backend or the bridge never refreshed the tab",
        || {
            tab.entries
                .get()
                .iter()
                .any(|entry| entry.path == "new_file.txt")
        },
    );

    // The facade's own session must agree -- not just the UI's separate
    // re-list -- proving this is a real, backend-committed mutation, not
    // a UI-side illusion.
    runtime.block_on(async {
        let snapshot = app
            .archive_snapshot(session_id)
            .await
            .expect("archive_snapshot must succeed");
        assert_eq!(
            snapshot.revision, 2,
            "one successful mutation must bump the revision exactly once"
        );
    });
}

/// The riskiest untested UI path before this test existed:
/// `FileOpsService::delete_files` resolves its path-string selection to
/// `EntryId`s via one `list_entries` call scoped to
/// `origin.navigation.get().current_path` -- every prior delete test
/// only ever exercised the root directory, where that scoping is
/// trivially correct (an empty `ArchivePath`). This proves it end to end
/// for a real subdirectory: the user has navigated into `subdir/`, the
/// selected path is `subdir/nested.txt` (the full archive-relative path,
/// matching what the browser's own selection always stores -- see
/// `FileOpsService::delete_files`'s own doc comment), and only that file
/// -- never the untouched root-level sibling -- must disappear.
#[test]
fn delete_files_from_a_subdirectory_resolves_through_the_navigated_directory_and_reaches_a_real_backend(
) {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("archive.zip");
    build_zip_fixture(
        &archive_path,
        &[
            ("root_level.txt", b"stays"),
            ("subdir/nested.txt", b"goes"),
            ("subdir/sibling.txt", b"also stays"),
        ],
    );

    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();
    arclain_ui::core::operation_bridge::spawn(&shared);

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on(async {
        let operation_id = app
            .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                source_path: archive_path.clone(),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        wait_for_open_completion(&app, operation_id).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
    });

    wait_until(
        "the archive open never populated the tab's entries signal",
        || tab.entries.get().len() == 3,
    );

    // Simulate the user having navigated into `subdir` before selecting
    // and deleting `nested.txt` -- `FileOpsService::delete_files` scopes
    // its own `list_entries` call to exactly this signal.
    tab.navigation.update(|nav| nav.set_current_path("subdir"));

    arclain_ui::features::archive_browser::application::FileOpsService.delete_files(
        &shared,
        tab.clone(),
        vec!["subdir/nested.txt".to_string()],
    );

    wait_until(
        "delete_files from a subdirectory never reached a real backend or the bridge never \
         refreshed the tab",
        || {
            let entries = tab.entries.get();
            entries.len() == 2
                && !entries
                    .iter()
                    .any(|entry| entry.path == "subdir/nested.txt")
        },
    );

    let final_entries = tab.entries.get();
    let remaining: std::collections::HashSet<&str> = final_entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert!(
        remaining.contains("root_level.txt"),
        "the untouched root-level file must survive"
    );
    assert!(
        remaining.contains("subdir/sibling.txt"),
        "the untouched sibling in the same subdirectory must survive"
    );
}

#[test]
fn start_add_files_is_a_no_op_without_an_application_facade() {
    let shared = create_test_shared_state();
    assert!(
        shared.facade.is_none(),
        "this fixture must not have a facade for this test to mean anything"
    );
    let tab_id = shared.signals().tabs.get().active_id();
    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);

    // Must return without panicking; there is nothing further to
    // observe since no facade means no operation is ever dispatched.
    arclain_ui::core::operations::file::start_add_files(
        &shared,
        tab_id,
        session_id,
        vec![std::path::PathBuf::from("whatever.txt")],
    );
}

#[test]
fn start_add_files_is_a_no_op_for_an_empty_source_list_even_with_a_facade_present() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app);
    let tab_id = shared.signals().tabs.get().active_id();
    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);

    // No archive_snapshot/start_archive_mutation round trip should even
    // be attempted for an empty source list -- if it were, this
    // deliberately-bogus session id would surface as a logged facade
    // error. Nothing to assert beyond "this returns promptly and does
    // not panic".
    arclain_ui::core::operations::file::start_add_files(&shared, tab_id, session_id, vec![]);
}

#[test]
fn delete_files_is_a_no_op_without_an_application_facade() {
    let shared = create_test_shared_state();
    assert!(shared.facade.is_none());
    let tab = shared.signals().tabs.get().active().clone();

    arclain_ui::features::archive_browser::application::FileOpsService.delete_files(
        &shared,
        tab,
        vec!["whatever.txt".to_string()],
    );
}

#[test]
fn delete_files_with_an_empty_selection_shows_a_status_message_and_never_touches_the_facade() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app);
    let tab = shared.signals().tabs.get().active().clone();

    arclain_ui::features::archive_browser::application::FileOpsService.delete_files(
        &shared,
        tab,
        vec![],
    );

    assert_eq!(
        shared.signals().status_bar.get().message,
        "No files selected"
    );
}

#[test]
fn start_replace_text_is_a_no_op_without_an_application_facade() {
    let shared = create_test_shared_state();
    assert!(shared.facade.is_none());
    let tab_id = shared.signals().tabs.get().active_id();
    let session_id = arclain_app::ids::ArchiveSessionId::from_raw(1);

    arclain_ui::core::operations::file::start_replace_text(
        &shared,
        tab_id,
        session_id,
        "readme.txt".to_string(),
        "new content".to_string(),
    );
}
