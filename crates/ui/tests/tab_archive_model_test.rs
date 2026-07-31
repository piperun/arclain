//! End-to-end coverage for a tab's facade-typed archive model: the
//! session id and [`ArchiveSnapshot`] an open stamps onto the tab, the
//! [`TabListing`] cursor it browses with, and the browser rows the
//! session's own entries convert into.
//!
//! The load-bearing assertion is an *equivalence* one, and it is what
//! licenses the render path's whole design. The browser draws a folder
//! by scoping the tab's whole-archive inventory to it
//! (`crate::core::operations::browser_rows::rows_in_directory`), because
//! that repaints instantly on navigation; the session's own answer to
//! the same question is `ArclainApp::list_entries` for that directory.
//! Every test below drives *both* against the same real archive and
//! asserts the rows match field for field -- that is what makes the
//! render path a scoping of the session's answer rather than a second
//! listing pipeline reconstructing one, and what would catch a silent
//! display regression (a lost Modified date, a folder whose recursive
//! size stopped aggregating) the moment the two drifted.
//!
//! A real, on-disk ZIP fixture rather than a fake backend: the rows both
//! producers consume are the ones a real backend listed and the facade's
//! session indexed, so the equivalence covers the whole conversion chain
//! (backend string shapes included), not a fixture's idealized values.
//!
//! [`ArchiveSnapshot`]: arclain_app::archive::ArchiveSnapshot
//! [`TabListing`]: arclain_ui::core::tabs::TabListing

mod common;
use common::create_test_shared_state;

use arclain_app::archive::{ArchivePath, EntryPage, ListEntriesRequest};
use arclain_app::ids::EntryId;
use arclain_ui::core::operations::browser_rows::{folder_paths, rows_in_directory};
use arclain_ui::core::tabs::{RequestStatus, TabListing, TabState};
use arclain_ui::core::utils::file_entry_from_dto;
use arclain_ui::shared::models::file_entry::FileEntry;
use arclain_ui::shared::SharedState;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A fixture with every shape the browser's row conversion has to get
/// right: a root-level file, a root-level directory the listing implies
/// rather than names, two nesting levels under it, an encrypted entry, and
/// mixed-case names (whose tie ordering the two producers deliberately
/// disagree on -- see [`sorted_by_archive_path`]).
///
/// Deliberately no *explicit* directory entry: that one shape is the only
/// place the two row producers genuinely disagree, and it gets its own
/// fixture and its own characterizing test
/// (`an_explicitly_listed_directory_gains_its_own_modified_date`) rather
/// than weakening every parity assertion here.
const FIXTURE: &[FixtureEntry] = &[
    FixtureEntry::file("readme.txt", b"readme"),
    FixtureEntry::file("Zebra.txt", b"zebra"),
    FixtureEntry::file("game/Game.exe", b"executable-bytes"),
    FixtureEntry::file("game/data/save.dat", b"save-data-contents"),
    FixtureEntry::file("game/data/config.ini", b"[settings]"),
    FixtureEntry::encrypted_file("game/licence.key", b"secret-licence-bytes"),
];

/// One entry [`build_zip_fixture`] writes. `Directory` exists so a fixture
/// can carry a directory the archive *names*, rather than only ones its
/// file paths imply -- 7z and rar emit those routinely.
#[derive(Clone, Copy)]
enum FixtureEntry {
    File {
        path: &'static str,
        content: &'static [u8],
        encrypted: bool,
    },
    Directory(&'static str),
}

impl FixtureEntry {
    const fn file(path: &'static str, content: &'static [u8]) -> Self {
        Self::File {
            path,
            content,
            encrypted: false,
        }
    }

    const fn encrypted_file(path: &'static str, content: &'static [u8]) -> Self {
        Self::File {
            path,
            content,
            encrypted: true,
        }
    }
}

/// The password the encrypted fixture entry is written with. Never needed
/// for *listing* -- a zip's central directory is readable without it, which
/// is why an encrypted entry can appear in a listing-parity fixture at all.
const FIXTURE_PASSWORD: &str = "fixture-password";

fn build_zip_fixture(path: &Path, entries: &[FixtureEntry]) {
    let file = std::fs::File::create(path).expect("create zip fixture file");
    let mut writer = zip::ZipWriter::new(file);
    let plain = zip::write::SimpleFileOptions::default();
    for entry in entries {
        match entry {
            FixtureEntry::Directory(name) => writer
                .add_directory(*name, plain)
                .expect("add zip fixture directory"),
            FixtureEntry::File {
                path: entry_path,
                content,
                encrypted,
            } => {
                let options = if *encrypted {
                    plain.with_aes_encryption(zip::AesMode::Aes256, FIXTURE_PASSWORD)
                } else {
                    plain
                };
                writer
                    .start_file(*entry_path, options)
                    .expect("start zip fixture entry");
                std::io::Write::write_all(&mut writer, content)
                    .expect("write zip fixture entry content");
            }
        }
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
    open_entries(FIXTURE)
}

fn open_entries(entries: &[FixtureEntry]) -> OpenedFixture {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("fixture.zip");
    build_zip_fixture(&archive_path, entries);

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
        "the archive open never populated the tab's inventory",
        || tab.inventory.get().entry_count() > 0,
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

/// The two producers order ties differently -- a page is sorted by the
/// lowercased name, the inventory is in depth-first tree order -- and
/// neither order is what the user sees, because the file list re-sorts
/// every row itself before rendering. Comparing sorted by the stable
/// archive-root path takes that difference out of the assertion without
/// weakening it: every row, and every field of every row, still has to
/// match.
fn sorted_by_archive_path(mut rows: Vec<FileEntry>) -> Vec<FileEntry> {
    rows.sort_by(|left, right| left.archive_path.cmp(&right.archive_path));
    rows
}

/// What the session answers for the browsed directory: one
/// `list_entries` call, converted row by row.
fn page_rows(fixture: &OpenedFixture, listing: &TabListing) -> Vec<FileEntry> {
    sorted_by_archive_path(
        list(fixture, listing.request())
            .entries
            .iter()
            .map(file_entry_from_dto)
            .collect(),
    )
}

/// What the renderer actually draws: the tab's whole-archive inventory
/// scoped to the browsed directory.
fn rendered_rows(fixture: &OpenedFixture, listing: &TabListing) -> Vec<FileEntry> {
    sorted_by_archive_path(rows_in_directory(
        fixture.tab.inventory.get().entries(),
        listing.directory(),
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
        // Every fixture file plus the two directories the index derives
        // from their paths: `game` and `game/data`.
        (FIXTURE.len() + 2) as u64
    );
    assert_eq!(
        snapshot.total_uncompressed_size,
        FIXTURE
            .iter()
            .map(|entry| match entry {
                FixtureEntry::File { content, .. } => content.len() as u64,
                FixtureEntry::Directory(_) => 0,
            })
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
    assert_eq!(
        listing.status(),
        &RequestStatus::Unlisted,
        "a fresh tab has asked for nothing, which is a different state from \
         having asked and been answered with nothing"
    );
    assert_eq!(tab.inventory.get().entry_count(), 0);
    assert!(tab.archive_snapshot.get().is_none());
    assert!(tab.archive_session_id.get().is_none());
}

/// The equivalence assertion this whole module exists for, at the root.
#[test]
fn the_rendered_root_matches_the_page_the_session_answers_with() {
    let fixture = open_fixture();
    let listing = TabListing::default();

    let from_page = page_rows(&fixture, &listing);
    let rendered = rendered_rows(&fixture, &listing);

    assert_eq!(
        from_page, rendered,
        "the session's own listing must render the archive root identically \
         to the flat filter the browser reads today"
    );
    assert_eq!(
        from_page
            .iter()
            .map(|row| row.archive_path.as_str())
            .collect::<Vec<_>>(),
        vec!["Zebra.txt", "game", "readme.txt"],
        "the root must show its two files plus the implied `game` folder"
    );
    let game = from_page
        .iter()
        .find(|row| row.archive_path == "game")
        .unwrap();
    assert!(game.is_folder);
    assert_ne!(
        game.size, "0 B",
        "a folder row must report its descendants' aggregate size"
    );
}

/// Same equivalence one and two levels down, where folder aggregation
/// and directory synthesis actually have something to get wrong.
#[test]
fn descending_lists_the_directory_the_cursor_moved_to() {
    let fixture = open_fixture();
    let mut listing = TabListing::default();

    assert!(listing.descend("game"));
    assert_eq!(listing.request().directory.as_str(), "game");
    assert_eq!(
        page_rows(&fixture, &listing),
        rendered_rows(&fixture, &listing)
    );
    let game_rows = page_rows(&fixture, &listing);
    assert_eq!(
        game_rows
            .iter()
            .map(|row| row.archive_path.as_str())
            .collect::<Vec<_>>(),
        vec!["game/Game.exe", "game/data", "game/licence.key"]
    );
    assert!(
        game_rows
            .iter()
            .find(|row| row.archive_path == "game/licence.key")
            .expect("the encrypted entry must be listed")
            .encrypted,
        "an encrypted entry must still be flagged as one after the DTO round trip"
    );

    assert!(listing.descend("data"));
    assert_eq!(listing.request().directory.as_str(), "game/data");
    assert_eq!(
        page_rows(&fixture, &listing),
        rendered_rows(&fixture, &listing)
    );
    assert_eq!(
        page_rows(&fixture, &listing)
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
        page_rows(&fixture, &listing),
        rendered_rows(&fixture, &listing)
    );

    assert!(listing.up());
    assert_eq!(listing.request().directory, ArchivePath::root());
    assert_eq!(
        page_rows(&fixture, &listing),
        rendered_rows(&fixture, &listing)
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

    let readme = page_rows(&fixture, &listing)
        .into_iter()
        .find(|row| row.archive_path == "readme.txt")
        .expect("the root listing must contain readme.txt");

    assert!(
        !readme.modified.is_empty(),
        "the zip backend reports a timestamp for every entry it lists"
    );
    assert_eq!(
        readme.modified,
        rendered_rows(&fixture, &listing)
            .into_iter()
            .find(|row| row.archive_path == "readme.txt")
            .unwrap()
            .modified,
        "the rendered timestamp must match the backend's own string byte for byte"
    );
}

/// The tab's listing is bound to the session the open stamped, and the
/// request it carries names the directory the cursor moved to -- so the
/// session's own answer to that request is the directory the user is
/// looking at, not the one the tab started on.
#[test]
fn a_tabs_listing_is_bound_to_its_session_and_requests_its_own_directory() {
    let fixture = open_fixture();

    let mut listing = fixture.tab.listing.get();
    assert_eq!(
        listing.session(),
        fixture.tab.archive_session_id.get(),
        "the open must have bound the tab's listing to the session it holds"
    );
    assert_eq!(
        listing.status(),
        &RequestStatus::Listed,
        "the open's own listing answered"
    );

    assert!(listing.descend("game"));
    let answered = list(&fixture, listing.request());
    assert_eq!(answered.directory.as_str(), "game");
    let mut paths: Vec<&str> = answered
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        vec!["game/Game.exe", "game/data", "game/licence.key"]
    );
}

/// The tree panel's folder set is every directory the session indexed --
/// including the ones no archive entry names, which it synthesized from
/// the paths beneath them. Nothing re-derives ancestors from file paths
/// on the render side.
#[test]
fn the_tree_folder_set_is_every_directory_the_session_indexed() {
    let fixture = open_fixture();

    assert_eq!(
        folder_paths(fixture.tab.inventory.get().entries()),
        vec!["game".to_string(), "game/data".to_string()],
        "both folders are implied by file paths -- neither is a named entry          in the fixture"
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

/// The one shape where the pre-facade backend re-list and the session's
/// rows genuinely disagreed -- and where deleting the duplicate pipeline
/// deliberately changed what the user sees.
///
/// When an archive *names* a directory (rather than only implying it
/// from its files' paths), the pre-facade flat filter over raw backend
/// rows never trimmed the trailing `/` the directory entry carried, so
/// `"docs/"` looked like a nested path and it synthesized a dateless
/// `docs` folder row. The session's index normalizes the slash away,
/// recognizes the row as the directory itself, and keeps the timestamp
/// the archive recorded -- so a named folder now shows its own Modified
/// date, the sanctioned "folder rows gain Modified dates" change. Both
/// of today's producers read the session's rows, so they agree on it by
/// construction.
#[test]
fn an_explicitly_listed_directory_shows_the_date_the_archive_recorded() {
    let fixture = open_entries(&[
        FixtureEntry::Directory("docs/"),
        FixtureEntry::file("docs/manual.txt", b"manual"),
    ]);
    let listing = TabListing::default();

    let from_page = page_rows(&fixture, &listing);
    let rendered = rendered_rows(&fixture, &listing);

    assert_eq!(from_page.len(), 1);
    assert_eq!(from_page[0].archive_path, "docs");
    assert!(from_page[0].is_folder);
    assert!(
        !from_page[0].modified.is_empty(),
        "the session keeps the date the archive recorded for the directory it named"
    );
    assert_eq!(
        from_page, rendered,
        "both producers now read the session's rows, so the folder's date -- and \
         every other field -- must agree"
    );
}
