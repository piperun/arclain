//! End-to-end coverage for a tab's facade-typed archive model: the
//! session id and [`ArchiveSnapshot`] an open stamps onto the tab, the
//! [`TabListing`] cursor it browses with, and the browser rows a
//! session's own [`EntryPage`] converts into.
//!
//! The load-bearing assertion is a *parity* one. The archive browser
//! still renders rows produced by the pre-facade flat-listing projection
//! (`crate::core::operations::navigation_view::rows_in_directory` over
//! `TabState::entries`); the facade path
//! (`ArclainApp::list_entries` + `crate::core::utils::file_entry_from_dto`)
//! is what replaces it. Every test below drives *both* against the same
//! real archive and asserts the rows match field for field -- that is
//! what makes swapping the producer a safe change rather than a hopeful
//! one, and what would catch a silent display regression (a lost
//! Modified date, a folder whose recursive size stopped aggregating) the
//! moment either side drifted.
//!
//! A real, on-disk ZIP fixture rather than a fake backend, for the same
//! reason `archive_mutation_ui_test.rs` documents: the UI's own re-list
//! resolves its backend through `SharedState`'s extension-based
//! `BackendSelector`, independently of whatever backend the facade was
//! bootstrapped with, so only a real archive keeps both sides looking at
//! the same bytes.
//!
//! [`ArchiveSnapshot`]: arclain_app::archive::ArchiveSnapshot
//! [`EntryPage`]: arclain_app::archive::EntryPage
//! [`TabListing`]: arclain_ui::core::tabs::TabListing

mod common;
use common::create_test_shared_state;

use arclain_app::archive::{ArchivePath, EntryPage, ListEntriesRequest};
use arclain_app::ids::EntryId;
use arclain_ui::core::operations::navigation_view::rows_in_directory;
use arclain_ui::core::tabs::{TabListing, TabState};
use arclain_ui::core::utils::file_entry_from_dto;
use arclain_ui::shared::models::file_entry::FileEntry;
use arclain_ui::shared::SharedState;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A fixture with every shape the browser's row conversion has to get
/// right: a root-level file, a root-level directory the listing implies
/// rather than names, two nesting levels under it, and mixed-case names
/// (whose tie ordering the two producers deliberately disagree on -- see
/// [`sorted_by_archive_path`]).
const FIXTURE: &[(&str, &[u8])] = &[
    ("readme.txt", b"readme"),
    ("Zebra.txt", b"zebra"),
    ("game/Game.exe", b"executable-bytes"),
    ("game/data/save.dat", b"save-data-contents"),
    ("game/data/config.ini", b"[settings]"),
];

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

/// One opened fixture archive, with the tab the bridge stamped and the
/// facade it was opened through. `_temp` must stay alive: dropping it
/// deletes both the fixture and the facade's databases.
struct OpenedFixture {
    _temp: tempfile::TempDir,
    app: arclain_app::ArclainApp,
    shared: SharedState,
    tab: Arc<TabState>,
}

/// Opens [`FIXTURE`] into the active tab through the real bridge, so the
/// tab ends up carrying exactly what production would give it: a session
/// id, a snapshot, and the flat entry list the browser reads today.
fn open_fixture() -> OpenedFixture {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("fixture.zip");
    build_zip_fixture(&archive_path, FIXTURE);

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

    wait_until(
        "the archive open never populated the tab's flat entries",
        || !tab.entries.get().is_empty(),
    );

    OpenedFixture {
        _temp: temp,
        app,
        shared,
        tab,
    }
}

fn list(fixture: &OpenedFixture, request: &ListEntriesRequest) -> EntryPage {
    let session_id = fixture
        .tab
        .archive_session_id
        .get()
        .expect("the open must have stamped a session onto the tab");
    fixture
        .shared
        .services
        .tokio_runtime
        .block_on(fixture.app.list_entries(session_id, request.clone()))
        .expect("list_entries must succeed for the tab's own session")
}

/// The two row producers order ties differently -- the flat filter keys
/// on the display-relative path (so `Zebra.txt` sorts before
/// `readme.txt`, uppercase first), the session on the lowercased name --
/// and neither order is what the user sees, because the file list
/// re-sorts every row itself before rendering. Comparing sorted by the
/// stable archive-root path takes that difference out of the assertion
/// without weakening it: every row, and every field of every row, still
/// has to match.
fn sorted_by_archive_path(mut rows: Vec<FileEntry>) -> Vec<FileEntry> {
    rows.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    rows
}

fn facade_rows(fixture: &OpenedFixture, listing: &TabListing) -> Vec<FileEntry> {
    sorted_by_archive_path(
        list(fixture, listing.request())
            .entries
            .iter()
            .map(file_entry_from_dto)
            .collect(),
    )
}

fn flat_listing_rows(fixture: &OpenedFixture, listing: &TabListing) -> Vec<FileEntry> {
    sorted_by_archive_path(rows_in_directory(
        &fixture.tab.entries.get(),
        listing.current_path(),
    ))
}

#[test]
fn opening_an_archive_stamps_the_tab_with_the_session_its_snapshot_describes() {
    let fixture = open_fixture();

    let session_id = fixture
        .tab
        .archive_session_id
        .get()
        .expect("the open must have stamped a session id");
    let snapshot = fixture
        .tab
        .archive_snapshot
        .get()
        .expect("the open must have stamped the snapshot alongside the session id");

    assert_eq!(
        snapshot.session_id, session_id,
        "the tab's snapshot must describe the session the tab holds"
    );
    assert_eq!(snapshot.archive_type, "zip");
    assert_eq!(
        snapshot.entry_count,
        // Five files plus the three directories the index derives from
        // their paths: `game`, `game/data`, and nothing else.
        (FIXTURE.len() + 2) as u64
    );
    assert_eq!(
        snapshot.total_uncompressed_size,
        FIXTURE
            .iter()
            .map(|(_, content)| content.len() as u64)
            .sum::<u64>()
    );
    assert_eq!(snapshot.revision, 1, "a freshly opened session starts at 1");
}

#[test]
fn a_fresh_tab_browses_the_archive_root_with_nothing_listed_yet() {
    let tab = TabState::new(arclain_ui::core::tabs::TabId(1));
    let listing = tab.listing.get();

    assert_eq!(listing.directory(), &ArchivePath::root());
    assert_eq!(listing.current_path(), "");
    assert!(listing.page().is_none());
    assert!(tab.archive_snapshot.get().is_none());
    assert!(tab.archive_session_id.get().is_none());
}

/// The parity assertion this whole module exists for, at the root.
#[test]
fn a_root_page_converts_to_the_same_rows_the_flat_listing_produced() {
    let fixture = open_fixture();
    let listing = TabListing::default();

    let from_facade = facade_rows(&fixture, &listing);
    let from_flat_listing = flat_listing_rows(&fixture, &listing);

    assert_eq!(
        from_facade, from_flat_listing,
        "the session's own listing must render the archive root identically \
         to the flat filter the browser reads today"
    );
    assert_eq!(
        from_facade
            .iter()
            .map(|row| row.archive_path.as_str())
            .collect::<Vec<_>>(),
        vec!["Zebra.txt", "game", "readme.txt"],
        "the root must show its two files plus the implied `game` folder"
    );
    let game = from_facade
        .iter()
        .find(|row| row.archive_path == "game")
        .unwrap();
    assert!(game.is_folder);
    assert_ne!(
        game.size, "0 B",
        "a folder row must report its descendants' aggregate size"
    );
}

/// Same parity assertion one and two levels down, where folder
/// aggregation and directory synthesis actually have something to get
/// wrong.
#[test]
fn descending_lists_the_directory_the_cursor_moved_to() {
    let fixture = open_fixture();
    let mut listing = TabListing::default();

    assert!(listing.descend("game"));
    assert_eq!(listing.request().directory.as_str(), "game");
    assert_eq!(
        facade_rows(&fixture, &listing),
        flat_listing_rows(&fixture, &listing)
    );
    assert_eq!(
        facade_rows(&fixture, &listing)
            .iter()
            .map(|row| row.archive_path.as_str())
            .collect::<Vec<_>>(),
        vec!["game/Game.exe", "game/data"]
    );

    assert!(listing.descend("data"));
    assert_eq!(listing.request().directory.as_str(), "game/data");
    assert_eq!(
        facade_rows(&fixture, &listing),
        flat_listing_rows(&fixture, &listing)
    );
    assert_eq!(
        facade_rows(&fixture, &listing)
            .iter()
            .map(|row| row.archive_path.as_str())
            .collect::<Vec<_>>(),
        vec!["game/data/config.ini", "game/data/save.dat"]
    );
}

#[test]
fn ascending_restores_the_parent_directorys_listing() {
    let fixture = open_fixture();
    let mut listing = TabListing::default();
    listing.descend("game/data");

    assert!(listing.up());
    assert_eq!(listing.request().directory.as_str(), "game");
    assert_eq!(
        facade_rows(&fixture, &listing),
        flat_listing_rows(&fixture, &listing)
    );

    assert!(listing.up());
    assert_eq!(listing.request().directory, ArchivePath::root());
    assert_eq!(
        facade_rows(&fixture, &listing),
        flat_listing_rows(&fixture, &listing)
    );
    assert!(!listing.can_go_up());
}

/// A zip entry's own timestamp has to survive being parsed into
/// `ArchiveEntryDto::modified_at_unix_ms` and rendered back out, or the
/// file list's Modified column silently empties (or reformats) the day
/// the browser starts reading pages.
#[test]
fn a_zip_entrys_modified_date_survives_the_trip_through_the_dto() {
    let fixture = open_fixture();
    let listing = TabListing::default();

    let readme = facade_rows(&fixture, &listing)
        .into_iter()
        .find(|row| row.archive_path == "readme.txt")
        .expect("the root listing must contain readme.txt");

    assert!(
        !readme.modified.is_empty(),
        "the zip backend reports a timestamp for every entry it lists"
    );
    assert_eq!(
        readme.modified,
        flat_listing_rows(&fixture, &listing)
            .into_iter()
            .find(|row| row.archive_path == "readme.txt")
            .unwrap()
            .modified,
        "the rendered timestamp must match the backend's own string byte for byte"
    );
}

/// A tab holding a page answers "what is in the folder on screen?" from
/// that page -- the same question the pre-facade flat filter answered.
#[test]
fn a_tab_holding_a_page_reports_that_page_as_its_current_directory() {
    let fixture = open_fixture();

    let mut listing = TabListing::default();
    assert!(listing.descend("game"));
    let page = list(&fixture, listing.request());
    assert!(listing.adopt_page(page));
    fixture.tab.listing.set(listing);

    let current = fixture.shared.app_state.lock().get_current_entries();
    let mut paths: Vec<&str> = current.iter().map(|entry| entry.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["game/Game.exe", "game/data"]);
    assert!(
        current
            .iter()
            .find(|entry| entry.path == "game/data")
            .unwrap()
            .is_dir
    );
}

/// Selection and expansion state that caches an `EntryId` must survive a
/// refresh: re-listing the same session -- whether the same directory
/// again or after navigating away and back -- has to hand back the ids it
/// already minted, never fresh ones.
#[test]
fn entry_ids_survive_a_refresh_within_the_same_session() {
    let fixture = open_fixture();
    let root = TabListing::default();

    let ids_of = |listing: &TabListing| -> Vec<(String, EntryId)> {
        let mut ids: Vec<(String, EntryId)> = list(&fixture, listing.request())
            .entries
            .iter()
            .map(|entry| (entry.path.as_str().to_string(), entry.id))
            .collect();
        ids.sort_by(|left, right| left.0.cmp(&right.0));
        ids
    };

    let first = ids_of(&root);
    assert_eq!(
        first,
        ids_of(&root),
        "re-listing the same directory minted new ids"
    );

    let mut wandered = TabListing::default();
    assert!(wandered.descend("game"));
    let nested = ids_of(&wandered);
    assert!(wandered.up());
    assert_eq!(
        first,
        ids_of(&wandered),
        "returning to the root after navigating away minted new ids"
    );
    assert!(wandered.descend("game"));
    assert_eq!(
        nested,
        ids_of(&wandered),
        "returning to a subdirectory minted new ids"
    );

    assert!(
        first
            .iter()
            .all(|(_, id)| !nested.iter().any(|(_, other)| other == id)),
        "two different entries were handed the same id within one session"
    );
}
