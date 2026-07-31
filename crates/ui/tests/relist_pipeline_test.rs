//! End-to-end coverage for the operation bridge's session-backed relist
//! pipeline -- the write path that feeds a tab's `TabListing`/
//! `TabInventory` from the facade session instead of the deleted
//! duplicate backend listing.
//!
//! Everything here drives the *real* pieces together: a real ZIP (or 7z)
//! fixture, a real bootstrapped `ArclainApp`, the real operation bridge,
//! and the real relist/refresh functions -- so the seams pinned in
//! isolation (`begin_loading`/`succeed`/`fail`, and the
//! keep-rows-on-failed-refresh semantics) are exercised through the same
//! code production runs.
//!
//! Assertions about what the user sees are stated against the tab's
//! published browser rows (`TabState::browser_entries`), because those
//! are what the browser draws -- and, paired with the listing's status,
//! what `browser_body` turns into a folder, a failure, or a spinner.

mod common;
use common::create_test_shared_state;

use arclain_app::archive::ArchivePath;
use arclain_ui::core::tabs::{RequestStatus, TabState};
use arclain_ui::features::archive_browser::presentation::{browser_body, BrowserBody};
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

/// The rows the browser is actually drawing for the folder on screen --
/// what a relist republishes, and the axis every assertion below about
/// "what the user sees" is stated against.
fn drawn_paths(tab: &TabState) -> Vec<String> {
    tab.browser_entries
        .get()
        .entries
        .iter()
        .map(|row| row.archive_path.clone())
        .collect()
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

    // Open seated the session's rows and drew the root from them.
    {
        let listing = tab.listing.get();
        assert_eq!(listing.session(), tab.archive_session_id.get());
        assert_eq!(listing.directory(), &ArchivePath::root());
        assert_eq!(listing.status(), &RequestStatus::Idle);
        assert_eq!(tab.inventory.get().revision(), Some(1));
        assert_eq!(
            tab.inventory.get().entry_count(),
            4,
            "three files plus the synthesized `subdir` directory"
        );
        assert_eq!(drawn_paths(tab), ["root.txt", "subdir"]);
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
    let drawn_revision_before = tab.browser_entries.get().revision;

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
    wait_until("the refresh never republished the browser's rows", || {
        tab.browser_entries.get().revision != drawn_revision_before
    });

    // The cursor stayed where the user was browsing, and the republished
    // rows answer *that* directory rather than the root the tab opened on
    // -- the added file landed at the root, so a root-scoped republish
    // would be visible here as three rows including `added.txt`.
    let listing = tab.listing.get();
    assert_eq!(listing.current_path(), "subdir");
    assert_eq!(listing.status(), &RequestStatus::Idle);
    assert_eq!(
        drawn_paths(tab),
        ["subdir/keep.txt", "subdir/second.txt"],
        "the refreshed rows answer the navigated directory"
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
            .entries()
            .iter()
            .any(|entry| entry.path.as_str() == "added.txt"),
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
        matches!(listing.status(), RequestStatus::Failed(_)),
        "the listing records the failure instead of masquerading as empty"
    );
    assert!(
        listing.failure().is_some(),
        "the failure envelope is observable for a renderer to offer a retry"
    );
    assert_eq!(
        tab.inventory.get().entry_count(),
        0,
        "nothing is known about the archive -- no rows were ever seated"
    );
    assert!(drawn_paths(&tab).is_empty());
    let body = browser_body(false, listing.status());
    assert!(
        matches!(&body, BrowserBody::Unlistable(failure)
            if failure.summary == listing.failure().unwrap().summary),
        "the browser must draw this as a failure carrying the error's own \
         summary, not as an empty archive: {body:?}"
    );
}

/// The auto-password ladder is the facade's alone now: a
/// header-encrypted archive whose password a stored rule knows opens end
/// to end -- real 7-Zip, real vault-seeded rule, no password typed, no
/// challenge raised -- and the tab comes out fully populated.
///
/// The password never reaches the frontend at all, and the file-edit
/// read still works anyway: the session supplies its own password to
/// `read_entry_text`. Both halves are asserted together because they are
/// one claim -- the UI stopped needing the secret precisely because the
/// read stopped being the UI's.
#[test]
fn a_rule_protected_archive_opens_and_reads_without_the_ui_holding_its_password() {
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
    let inventory = tab.inventory.get();
    let payload = inventory
        .entries()
        .iter()
        .find(|entry| entry.path.as_str() == "payload.txt")
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
    assert!(
        tab.current_password.get().is_none(),
        "the archive's password stays in the session that resolved it -- \
         no rule-supplied secret is copied onto the tab"
    );

    // ...and the read the tab used to need that copy for still works,
    // because the session supplies its own password to the facade.
    arclain_ui::features::archive_browser::application::FileOpsService.read_text(
        &shared,
        tab.clone(),
        "payload.txt".to_string(),
    );
    wait_until("the file-edit read never completed", || {
        !matches!(
            tab.file_edit_dialog.get().load_state,
            arclain_ui::features::file_editing::domain::types::FileEditLoadState::Loading { .. }
        )
    });
    let dialog = tab.file_edit_dialog.get();
    assert_eq!(
        dialog.load_state,
        arclain_ui::features::file_editing::domain::types::FileEditLoadState::Ready,
        "reading an entry of a rule-unlocked archive failed: {}",
        dialog.error
    );
    assert_eq!(dialog.content, "locked payload");
}

/// A failed *refresh* keeps the rows already on screen and records the
/// failure alongside them -- the keep-the-rows semantics part 1 pinned on
/// the type, now exercised through the real refresh path against a real
/// session that disappears mid-flight.
///
/// This is the one case where rows and a recorded failure legitimately
/// coexist: the rows are the *same* archive's last good answer, so the
/// browser marks them stale rather than discarding them. A failed *open*
/// is the opposite case and gets the opposite treatment (see
/// [`a_failed_open_on_a_reused_tab_draws_neither_the_previous_archive_nor_an_empty_one`]).
#[test]
fn a_failed_refresh_keeps_the_rows_and_records_the_failure() {
    let fixture = open_fixture(&[("a.txt", b"content"), ("b.txt", b"more")]);
    let tab = &fixture.tab;
    let session_id = tab.archive_session_id.get().unwrap();
    let rows_before = drawn_paths(tab);
    assert_eq!(rows_before, ["a.txt", "b.txt"]);

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
        drawn_paths(tab),
        rows_before,
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

    let body = browser_body(true, listing.status());
    assert!(
        matches!(&body, BrowserBody::StaleListing(failure)
            if failure.summary == listing.failure().unwrap().summary),
        "the browser must keep drawing the rows and say why they are stale: {body:?}"
    );
}

/// The regression guard for the confident lie this whole two-axis model
/// exists to make impossible.
///
/// Every reused-tab route -- toolbar Open, Ctrl+O, a nested open, a
/// password-retry reopen -- re-points the *same* tab at a new archive.
/// The relist stamps the new archive's name onto the tab before it can
/// know whether the listing will succeed, so if the listing then fails
/// and nothing clears the published rows, the browser shows archive A's
/// rows under archive B's name. Those rows are not merely stale: they
/// describe a different archive entirely.
///
/// The fix has to be both halves at once. Clearing the rows alone would
/// trade the wrong rows for a bare empty folder -- the silent-empty-view
/// this model was reshaped to prevent -- so the failure has to be drawn
/// as a failure.
#[test]
fn a_failed_open_on_a_reused_tab_draws_neither_the_previous_archive_nor_an_empty_one() {
    let fixture = open_fixture(&[("first-archive.txt", b"a"), ("also-first.txt", b"b")]);
    let tab = &fixture.tab;
    let shared = &fixture.shared;
    assert_eq!(
        drawn_paths(tab),
        ["also-first.txt", "first-archive.txt"],
        "the first archive's rows are what the browser is drawing"
    );
    let first_path = tab
        .archive_path
        .get()
        .expect("the first open named the tab");

    // A second archive, opened into the same tab, whose session vanishes
    // between the facade's completion and the bridge's registration --
    // every listing fetch the relist makes will fail against it.
    let second_path = fixture._temp.path().join("second.zip");
    build_zip_fixture(&second_path, &[("second-archive.txt", b"c")]);
    let runtime = shared.services.tokio_runtime.clone();
    runtime.block_on({
        let shared = shared.clone();
        let app = fixture.app.clone();
        let second_path = second_path.clone();
        let tab_id = tab.id;
        async move {
            let operation_id = app
                .start_open_archive(arclain_app::archive::OpenArchiveRequest {
                    source_path: second_path,
                    password: None,
                })
                .await
                .expect("start_open_archive must be accepted");
            let snapshot = wait_for_open_completion(&app, operation_id).await;
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

    // The tab now names the second archive...
    let named = tab
        .archive_path
        .get()
        .expect("the tab still names an archive");
    assert_ne!(
        named, first_path,
        "the tab was re-pointed at the second archive"
    );
    assert_eq!(named, second_path);

    // ...so the first archive's rows must be gone. This is the half that
    // bites: without the clear, `drawn_paths` still answers with the
    // first archive's two rows under the second archive's name.
    assert!(
        drawn_paths(tab).is_empty(),
        "the previous archive's rows are still on screen under the new \
         archive's name: {:?}",
        drawn_paths(tab)
    );
    assert_eq!(tab.inventory.get().entry_count(), 0);

    // ...and the other half: what is on screen instead is the failure,
    // not an empty archive.
    let listing = tab.listing.get();
    let failure = listing
        .failure()
        .expect("the failed listing must be recorded on the tab it failed for");
    let body = browser_body(false, listing.status());
    assert!(
        matches!(&body, BrowserBody::Unlistable(drawn) if drawn.summary == failure.summary),
        "clearing the rows must not leave a bare empty folder behind: {body:?}"
    );
    assert_ne!(
        body,
        BrowserBody::Listing,
        "an empty `Listing` here is exactly the silent-empty-view this model prevents"
    );
}

/// A file and a directory sharing a name at the same level is legal in a
/// ZIP, and the session's entry index carries both rows. Reading the file
/// for editing must resolve to the *file*.
///
/// What this adds over the selection rule's own unit tests is that the
/// collision is *reachable*: a real ZIP really does produce two rows
/// answering to `notes`, and the whole resolution -- page fetch, row
/// selection, id hand-off, the session's own read -- survives it. It is
/// deliberately not the regression guard for the selection rule (a real
/// page currently orders the file first, so it would pass either way);
/// `readable_entry_named`'s unit tests are, and they bite.
#[test]
fn reading_a_file_whose_name_a_directory_shares_resolves_to_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("collision.zip");
    // `notes` is a file; `notes/inner.txt` implies a `notes` directory.
    build_zip_fixture(
        &archive_path,
        &[
            ("notes", b"the file's own contents" as &[u8]),
            ("notes/inner.txt", b"nested"),
        ],
    );

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
            wait_for_open_completion(&app, operation_id).await;
            arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id)
                .await;
        }
    });

    wait_until("the collision fixture never populated the tab", || {
        tab.inventory.get().entry_count() > 0
    });
    // Both rows really are there -- otherwise this test would pass by
    // never reproducing the collision at all.
    let inventory = tab.inventory.get();
    let named_notes: Vec<arclain_app::archive::EntryKind> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.path.as_str() == "notes")
        .map(|entry| entry.kind.clone())
        .collect();
    assert_eq!(
        named_notes.len(),
        2,
        "the fixture must produce a colliding file/directory pair, got {named_notes:?}"
    );

    arclain_ui::features::archive_browser::application::FileOpsService.read_text(
        &shared,
        tab.clone(),
        "notes".to_string(),
    );
    wait_until("the file-edit read never completed", || {
        !matches!(
            tab.file_edit_dialog.get().load_state,
            arclain_ui::features::file_editing::domain::types::FileEditLoadState::Loading { .. }
        )
    });

    let dialog = tab.file_edit_dialog.get();
    assert_eq!(
        dialog.load_state,
        arclain_ui::features::file_editing::domain::types::FileEditLoadState::Ready,
        "the directory row won the name match: {}",
        dialog.error
    );
    assert_eq!(dialog.content, "the file's own contents");
}
