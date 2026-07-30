//! End-to-end coverage for the operation bridge's session-backed relist
//! pipeline -- the write path that feeds a tab's `TabListing`/
//! `TabInventory` from the facade session instead of the deleted
//! duplicate backend listing.
//!
//! Everything here drives the *real* pieces together: a real ZIP (or 7z)
//! fixture, a real bootstrapped `ArclainApp`, the real operation bridge,
//! and the real relist/refresh functions -- so the seams part 1 pinned in
//! isolation (`begin_loading`/`adopt_page`/`fail`, and the
//! keep-rows-on-failed-refresh semantics) are exercised through the same
//! code production runs.

mod common;
use common::create_test_shared_state;

use arclain_app::archive::ArchivePath;
use arclain_ui::core::tabs::{RequestStatus, TabState};
use arclain_ui::shared::SharedState;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn build_zip_fixture(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_path, content) in entries {
        writer
            .start_file(*entry_path, options)
            .expect("start zip fixture entry");
        std::io::Write::write_all(&mut writer, content).expect("write zip fixture entry");
    }
    writer.finish().expect("finish zip fixture");
}

fn bootstrap_real_app(temp: &tempfile::TempDir) -> arclain_app::ArclainApp {
    arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        }),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap the test facade")
}

async fn wait_for_open_completion(
    app: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let operation = app.operation(operation_id).await.unwrap();
        match operation.state {
            arclain_app::event::OperationState::Completed {
                result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
            } => return snapshot,
            arclain_app::event::OperationState::Failed { error } => {
                panic!("archive open unexpectedly failed: {error:?}")
            }
            _ => {}
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

struct OpenedFixture {
    _temp: tempfile::TempDir,
    app: arclain_app::ArclainApp,
    shared: SharedState,
    tab: Arc<TabState>,
}

/// Opens a fixture into the active tab through the real bridge worker
/// (spawned, so mutation events route back too).
fn open_fixture(entries: &[(&str, &[u8])]) -> OpenedFixture {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("fixture.zip");
    build_zip_fixture(&archive_path, entries);

    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();
    arclain_ui::core::operation_bridge::spawn(&shared);

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on({
        let shared = shared.clone();
        let app = app.clone();
        let archive_path = archive_path.clone();
        async move {
            let operation_id = app
                .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                    source_path: archive_path,
                    password: None,
                })
                .await
                .expect("start_open_archive must be accepted");
            wait_for_open_completion(&app, operation_id).await;
            arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                .await;
        }
    });

    wait_until(
        "the archive open never seated the session's rows onto the tab",
        || tab.inventory.get().entry_count() > 0,
    );

    OpenedFixture {
        _temp: temp,
        app,
        shared,
        tab,
    }
}

/// The brief's end-to-end sequence: open, list, navigate, mutate,
/// relist -- with the selection and folder-expansion state surviving the
/// refresh, the refreshed page answering the *navigated* directory (not
/// the root), and the tab's snapshot tracking the bumped revision.
#[test]
fn a_mutation_relists_the_navigated_directory_and_selection_survives() {
    let fixture = open_fixture(&[
        ("root.txt", b"root"),
        ("subdir/keep.txt", b"keep me"),
        ("subdir/second.txt", b"second"),
    ]);
    let tab = &fixture.tab;

    // Open seated the root page from the session.
    {
        let listing = tab.listing.get();
        assert_eq!(listing.session(), tab.archive_session_id.get());
        let page = listing.page().expect("the open must seat the root page");
        assert_eq!(page.directory, ArchivePath::root());
        assert_eq!(page.revision, 1);
        assert_eq!(listing.status(), &RequestStatus::Idle);
        assert_eq!(
            tab.inventory.get().entry_count(),
            4,
            "three files plus the synthesized `subdir` directory"
        );
    }

    // The user browses into `subdir`, selects a file, and has the tree
    // expanded.
    tab.listing.update(|listing| {
        assert!(listing.go_to("subdir"));
    });
    {
        let mut view_state = tab.browser_view_state.get();
        view_state.selection.insert("subdir/keep.txt".to_string());
        view_state.tree_state.selected_path = "subdir".to_string();
        tab.browser_view_state.set(view_state);
    }
    let tree_state_before = tab.browser_view_state.get().tree_state.clone();

    // A real mutation through the facade, routed back by the real bridge.
    let new_file = fixture._temp.path().join("added.txt");
    std::fs::write(&new_file, b"added content").unwrap();
    let session_id = tab.archive_session_id.get().unwrap();
    arclain_ui::core::operations::file::start_add_files(
        &fixture.shared,
        tab.id,
        session_id,
        vec![new_file],
    );

    wait_until(
        "the mutation never relisted the tab through the bridge",
        || tab.inventory.get().revision() == Some(2),
    );
    wait_until(
        "the refresh never seated the navigated directory's fresh page",
        || {
            let listing = tab.listing.get();
            listing
                .page()
                .is_some_and(|page| page.revision == 2 && page.directory.as_str() == "subdir")
        },
    );

    // The cursor stayed where the user was browsing.
    let listing = tab.listing.get();
    assert_eq!(listing.current_path(), "subdir");
    assert_eq!(
        listing
            .page()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["subdir/keep.txt", "subdir/second.txt"],
        "the refreshed page answers the navigated directory"
    );

    // Selection and expansion survived the refresh.
    let view_state = tab.browser_view_state.get();
    assert!(
        view_state
            .selection
            .iter()
            .any(|path| path == "subdir/keep.txt"),
        "a selection on a surviving entry must not be pruned"
    );
    assert_eq!(
        view_state.tree_state, tree_state_before,
        "the tree panel's expansion state is untouched by a refresh"
    );

    // The snapshot signal tracked the mutation's revision bump.
    wait_until(
        "the snapshot signal never refreshed after the mutation",
        || {
            tab.archive_snapshot
                .get()
                .is_some_and(|snapshot| snapshot.revision == 2)
        },
    );

    // And the whole-archive inventory gained the added file.
    assert!(
        tab.inventory
            .get()
            .legacy_rows()
            .iter()
            .any(|entry| entry.path == "added.txt"),
        "the inventory must reflect the mutation"
    );
}

/// A listing that fails at open time must reach the user (the status
/// bar) and leave the listing observably *failed* -- not render as an
/// ordinary empty directory. Driven through the real completion handler:
/// the session is closed between the facade's completion and the
/// bridge's registration, so the relist's fetches fail exactly the way
/// any post-completion failure would.
#[test]
fn a_listing_that_fails_at_open_reaches_the_status_bar_not_an_empty_folder() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("fixture.zip");
    build_zip_fixture(&archive_path, &[("a.txt", b"content")]);

    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on({
        let shared = shared.clone();
        let app = app.clone();
        async move {
            let operation_id = app
                .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                    source_path: archive_path,
                    password: None,
                })
                .await
                .expect("start_open_archive must be accepted");
            let snapshot = wait_for_open_completion(&app, operation_id).await;
            // The session vanishes before the bridge handles the
            // completion -- every listing fetch the relist makes will
            // fail against the closed session.
            app.close_archive(snapshot.session_id)
                .await
                .expect("closing the fresh session must succeed");
            arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                .await;
        }
    });

    wait_until("the failed listing never reached the status bar", || {
        shared
            .signals()
            .status_bar
            .get()
            .message
            .contains("Archive opened but failed to display")
    });

    let listing = tab.listing.get();
    assert!(
        listing.page().is_none(),
        "nothing is known about the directory -- no page was ever seated"
    );
    assert!(
        matches!(listing.status(), RequestStatus::Failed(_)),
        "the listing records the failure instead of masquerading as empty"
    );
    assert!(
        listing.failure().is_some(),
        "the failure envelope is observable for a renderer to offer a retry"
    );
    assert_eq!(tab.inventory.get().entry_count(), 0);
}

/// The auto-password ladder is the facade's alone now: a
/// header-encrypted archive whose password a stored rule knows opens end
/// to end -- real 7-Zip, real vault-seeded rule, no password typed, no
/// challenge raised -- and the tab comes out fully populated, with the
/// resolved password stamped from the session's own handle (there is no
/// second, UI-side ladder left to re-derive it).
#[test]
fn a_rule_protected_archive_opens_through_the_facades_own_ladder() {
    const FIXTURE_PASSWORD: &str = "rule-supplied-password";

    let temp = tempfile::tempdir().unwrap();

    // A real header-encrypted 7z: without the password even the entry
    // names are unreadable, which is exactly the shape that forces the
    // open-time ladder (list fails -> consult stored rules -> retry).
    let content_dir = temp.path().join("content");
    std::fs::create_dir_all(&content_dir).unwrap();
    std::fs::write(content_dir.join("payload.txt"), b"locked payload").unwrap();
    let archive_path = temp.path().join("RJ123456.7z");
    let sevenzip = arclain_core::backends::SevenZipCli::detect(None)
        .expect("these tests require a real 7-Zip executable");
    let status = std::process::Command::new(sevenzip.exe_path())
        .arg("a")
        .arg(format!("-p{FIXTURE_PASSWORD}"))
        .arg("-mhe=on")
        .arg(&archive_path)
        .arg(content_dir.join("payload.txt"))
        .status()
        .expect("run 7z to build the encrypted fixture");
    assert!(status.success(), "7z must build the fixture");

    // Seed one enabled rule into the vault the bootstrap below will
    // load -- the same files `arclain_app`'s own bootstrap reads.
    {
        let secrets_dir = temp.path().join("data").join("secrets");
        std::fs::create_dir_all(&secrets_dir).unwrap();
        let key_path = secrets_dir.join("master.key");
        let key = arclain_core::SecretsKey::generate();
        key.save_to_file(&key_path).unwrap();
        let databases_dir = temp.path().join("data").join("databases");
        std::fs::create_dir_all(&databases_dir).unwrap();
        let db_paths = arclain_core::DbPaths {
            config_db: databases_dir.join("config.sqlite"),
            cache_db: databases_dir.join("metadata.sqlite"),
            secrets_db: secrets_dir.join("pass.redb"),
            key_file: Some(key_path),
        };
        let dbs = arclain_core::open_databases(&db_paths, &key).unwrap();
        dbs.secrets
            .replace_all_pass_rules(&[arclain_core::DbPassRule {
                name: "fixture rule".to_string(),
                pattern: "RJ123456".to_string(),
                password: FIXTURE_PASSWORD.to_string(),
                priority: 10,
                enabled: true,
            }])
            .unwrap();
    }

    let app = bootstrap_real_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    runtime.block_on({
        let shared = shared.clone();
        let app = app.clone();
        let archive_path = archive_path.clone();
        async move {
            let operation_id = app
                .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                    source_path: archive_path,
                    password: None,
                })
                .await
                .expect("start_open_archive must be accepted");
            // The ladder must resolve the rule itself -- a raised
            // challenge would park this forever, so completion within
            // the deadline IS the assertion that no prompt was needed.
            wait_for_open_completion(&app, operation_id).await;
            arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                .await;
        }
    });

    wait_until("the rule-unlocked archive never populated the tab", || {
        tab.inventory.get().entry_count() > 0
    });

    assert!(
        !tab.password_dialog.get().show,
        "no password dialog: the stored rule answered before any challenge"
    );
    let rows = tab.inventory.get().legacy_rows();
    let payload = rows
        .iter()
        .find(|entry| entry.path == "payload.txt")
        .expect("the unlocked listing's real entries reached the tab");
    assert!(
        payload.encrypted,
        "the entry keeps its encryption flag through the unlocked listing"
    );
    // (Deliberately no assertion on `snapshot.headers_encrypted`: it
    // mirrors whatever the *successful, unlocked* listing reported, and
    // whether 7-Zip's `-slt` output flags header encryption on an
    // already-unlocked listing varies by 7-Zip version -- the same
    // value the pre-facade extras carried, preserved, not redefined.)
    assert_eq!(
        tab.current_password.get().as_deref(),
        Some(FIXTURE_PASSWORD),
        "the resolved password is stamped from the session's own handle -- \
         nothing UI-side re-derived it"
    );
}

/// A failed *refresh* keeps the rows already on screen and records the
/// failure alongside them -- the keep-the-rows semantics part 1 pinned on
/// the type, now exercised through the real refresh path against a real
/// session that disappears mid-flight.
#[test]
fn a_failed_refresh_keeps_the_rows_and_records_the_failure() {
    let fixture = open_fixture(&[("a.txt", b"content"), ("b.txt", b"more")]);
    let tab = &fixture.tab;
    let session_id = tab.archive_session_id.get().unwrap();
    let rows_before = tab.listing.get().page().unwrap().entries.len();
    assert!(rows_before > 0);

    let runtime = fixture.shared.services.tokio_runtime.clone();
    runtime.block_on({
        let shared = fixture.shared.clone();
        let app = fixture.app.clone();
        let tab = tab.clone();
        async move {
            app.close_archive(session_id)
                .await
                .expect("closing the session must succeed");
            let error = arclain_ui::core::operation_bridge::refresh_entries_after_mutation(
                &shared, &tab, session_id,
            )
            .await
            .expect_err("refreshing a closed session must fail");
            assert_eq!(
                error.kind,
                arclain_app::error::ApplicationErrorKind::NotFound
            );
        }
    });

    let listing = tab.listing.get();
    assert_eq!(
        listing.page().map(|page| page.entries.len()),
        Some(rows_before),
        "the rows on screen are the session's last good answer and must survive"
    );
    assert!(
        matches!(listing.status(), RequestStatus::Failed(_)),
        "the failure is recorded alongside the kept rows"
    );
    assert!(
        tab.inventory.get().entry_count() > 0,
        "the whole-archive inventory keeps its last good answer too"
    );
}
