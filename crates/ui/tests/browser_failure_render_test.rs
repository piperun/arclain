//! What the archive browser actually draws for each combination of the
//! two axes `TabListing` keeps apart -- the rows published for the folder
//! on screen, and what the fetch behind them is doing.
//!
//! The model was reshaped so a *failed* listing could not be mistaken for
//! an empty folder, and for a while nothing read it: the browser drew
//! rows and only rows, so a failure looked exactly like an empty archive.
//! These tests pin the reading end. `browser_body` is asserted directly
//! (it is the whole decision, and a pure function of the two axes), and
//! each case is then rendered through the real `render_archive_browser`
//! so the decision demonstrably reaches a frame rather than only a match
//! arm.
//!
//! Kept in its own file rather than appended to `archive_browser_test.rs`
//! -- concurrent worktrees also edit that one.

mod common;

use arclain_app::archive::ArchivePath;
use arclain_app::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use arclain_app::ids::ArchiveSessionId;
use arclain_ui::core::tabs::{RequestStatus, TabListing, TabState};
use arclain_ui::features::archive_browser::presentation::{browser_body, BrowserBody};
use arclain_ui::features::archive_browser::ArchiveBrowser;
use arclain_ui::shared::models::file_entry::FileEntry;
use arclain_ui::shared::SharedState;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use std::path::PathBuf;
use std::sync::Arc;

const SESSION: u64 = 1;

/// The wording that only [`render_unlistable_state`]'s panel draws, and
/// the one assertion that separates "the failure is on screen" from "an
/// empty file list is on screen".
const UNLISTABLE_HEADLINE: &str = "Couldn't list this archive";
/// The wording that only the stale-rows banner draws.
const STALE_HEADLINE: &str = "Couldn't refresh";
/// The wording that only the in-flight panel draws.
const LOADING_HEADLINE: &str = "Listing the archive";
/// The tree panel's own heading -- drawn whenever that panel renders at
/// all, whatever folder set it ends up with.
const TREE_PANEL_HEADING: &str = "ARCHIVE STRUCTURE";
/// The file list's own column header, drawn even over zero rows -- so
/// "the file list is on screen" is observable without any row to look for.
/// The one positive marker that separates a drawn empty folder from a
/// central panel that drew something else entirely.
const FILE_LIST_COLUMN: &str = "Name";

fn listing_error(summary: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, summary)
}

fn row(name: &str) -> FileEntry {
    FileEntry {
        name: name.to_string(),
        path: name.to_string(),
        archive_path: name.to_string(),
        size: "0 B".to_string(),
        compressed: "0 B".to_string(),
        ratio: "0%".to_string(),
        modified: String::new(),
        crc32: String::new(),
        encrypted: false,
        is_folder: false,
    }
}

/// A tab that names an archive (so the browser draws a listing rather
/// than its "no archive loaded" state), holding `rows` and whatever
/// `status` the caller wants the listing to be in.
///
/// Every state but the first is driven through the real seams --
/// `begin_loading` then `succeed`/`fail` -- rather than constructed, so a
/// test can only express states the model can actually reach. `Unlisted`
/// is the one that needs no seam: it is what a fresh listing already is,
/// which is exactly the point.
fn seeded_tab(shared: &SharedState, rows: Vec<FileEntry>, status: &RequestStatus) -> Arc<TabState> {
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("fixture.zip")));

    let session_id = ArchiveSessionId::from_raw(SESSION);
    let mut listing = TabListing::for_session(Some(session_id));

    fn answer(listing: &mut TabListing, session_id: ArchiveSessionId) {
        let generation = listing.begin_loading();
        assert!(listing.succeed(generation, session_id, &ArchivePath::root()));
    }
    fn refuse(listing: &mut TabListing, session_id: ArchiveSessionId, error: &ApplicationError) {
        let generation = listing.begin_loading();
        assert!(listing.fail(generation, session_id, &ArchivePath::root(), error.clone()));
    }

    match status {
        RequestStatus::Unlisted => {}
        RequestStatus::Loading => {
            listing.begin_loading();
        }
        RequestStatus::Listed => answer(&mut listing, session_id),
        RequestStatus::Refreshing => {
            answer(&mut listing, session_id);
            listing.begin_loading();
        }
        RequestStatus::Unlistable(error) => refuse(&mut listing, session_id, error),
        RequestStatus::Stale(error) => {
            answer(&mut listing, session_id);
            refuse(&mut listing, session_id, error);
        }
    }
    assert_eq!(listing.status(), status);
    tab.listing.set(listing);

    tab.browser_entries
        .update(|snapshot| snapshot.replace(rows));
    tab
}

/// Seats a whole-archive inventory onto `tab` -- what the bridge's relist
/// adopts from `list_all_entries` in production, and what both side
/// panels project. Test-constructed `EntryId`s are fine: nothing here
/// hands one back to a facade.
fn seed_inventory(tab: &TabState) {
    use arclain_app::archive::{ArchiveEntryDto, ArchiveInventory, EntryKind};
    use arclain_app::ids::EntryId;
    use arclain_ui::core::tabs::{AdoptedInventory, TabInventory};

    let session_id = ArchiveSessionId::from_raw(SESSION);
    let rows = [
        ("manuals", EntryKind::Directory),
        ("manuals/a.txt", EntryKind::File),
    ];
    let inventory = ArchiveInventory {
        session_id,
        revision: 1,
        entries: rows
            .iter()
            .enumerate()
            .map(|(index, (path, kind))| ArchiveEntryDto {
                id: EntryId::from_raw(index as u64 + 1),
                path: ArchivePath::parse((*path).to_string()).unwrap(),
                name: path.rsplit('/').next().unwrap_or(path).to_string(),
                kind: kind.clone(),
                compressed_size: Some(0),
                uncompressed_size: 0,
                modified_at_unix_ms: None,
                encrypted: false,
                crc32: None,
            })
            .collect(),
    };
    let prepared = AdoptedInventory::prepare(inventory);
    tab.inventory.update(|held| {
        *held = TabInventory::for_session(Some(session_id));
        assert!(held.adopt(prepared.clone()));
    });
}

/// Runs the real browser view for a couple of frames against the active
/// tab, so the assertions below are about a drawn frame rather than a
/// model only the test looks at.
fn render(shared: &SharedState) -> Harness<'static, ()> {
    let shared = shared.clone();
    let mut harness = Harness::new(move |ctx| {
        let mut browser = ArchiveBrowser::new(&shared);
        browser.render(ctx, &shared);
    });
    // Stepped rather than `run`: the in-flight state draws a spinner,
    // which requests a repaint every frame by design, and `run` treats a
    // frame that never settles as a failure.
    harness.step();
    harness.step();
    harness
}

fn drew(harness: &Harness<'static, ()>, text: &str) -> bool {
    harness.query_by_label_contains(text).is_some()
}

// =========================================================================
// browser_body -- the decision itself
// =========================================================================

/// The distinction the whole model exists for. Nothing on screen means
/// entirely different things depending on the status, and only one of them
/// is "this folder is empty".
///
/// The first two assertions are the ones this file was extended for: an
/// archive nobody has listed and a directory the session answered as empty
/// publish exactly the same rows -- none -- so the row count cannot tell
/// them apart and nothing else was being asked.
#[test]
fn nothing_on_screen_means_empty_only_when_the_listing_actually_answered() {
    assert_eq!(
        browser_body(&RequestStatus::Unlisted),
        BrowserBody::Loading,
        "nothing has been asked yet -- drawing a folder here claims an \
         answer nobody has given"
    );
    assert_ne!(
        browser_body(&RequestStatus::Unlisted),
        browser_body(&RequestStatus::Listed),
        "an archive nobody listed must not draw as one listed and found empty"
    );
    assert_eq!(
        browser_body(&RequestStatus::Listed),
        BrowserBody::Listing,
        "the session answered with nothing: the folder really is empty"
    );
    assert_eq!(
        browser_body(&RequestStatus::Loading),
        BrowserBody::Loading,
        "nothing has answered yet -- an empty folder would be a guess"
    );
    assert!(
        matches!(
            browser_body(&unlistable(listing_error("the session is gone"))),
            BrowserBody::Unlistable(_)
        ),
        "the listing failed with nothing ever answered -- the contents are \
         unknown, not empty"
    );
}

/// Contents already answered stay drawn in every state; what changes is
/// whether a reason for distrusting them is drawn alongside. True of a
/// directory whose answer was "nothing here" as much as of one with rows
/// -- which is why the decision is not read off a row count.
#[test]
fn answered_contents_are_always_drawn_and_only_a_failure_marks_them_stale() {
    assert_eq!(browser_body(&RequestStatus::Listed), BrowserBody::Listing);
    assert_eq!(
        browser_body(&RequestStatus::Refreshing),
        BrowserBody::Listing,
        "a refresh in flight over an answer draws no banner: it is still the \
         last good one, and a banner per mutation is noise"
    );
    assert!(matches!(
        browser_body(&stale(listing_error("refresh failed"))),
        BrowserBody::StaleListing(_)
    ));
}

/// The failure the panel draws carries the envelope's own summary and the
/// action it suggests -- not a generic "something went wrong". The
/// suggestion is the field that exists precisely so a frontend need not
/// parse prose, and "supply a password" and "try again" are different
/// answers.
#[test]
fn the_drawn_failure_carries_the_envelopes_summary_and_suggested_action() {
    let error = listing_error("the archive is encrypted")
        .with_suggested_action(SuggestedAction::SupplyPassword);
    let BrowserBody::Unlistable(failure) = browser_body(&unlistable(error)) else {
        panic!("a listing that failed with nothing answered must draw as unlistable");
    };
    assert_eq!(failure.summary, "the archive is encrypted");
    assert_eq!(failure.hint, Some("This archive needs a password."));
}

/// With no suggested action the envelope still says whether trying again
/// could work, and a `Fatal` failure gets no invented advice.
#[test]
fn recoverability_stands_in_for_a_missing_suggested_action() {
    let retryable =
        listing_error("the backend timed out").with_recoverability(Recoverability::Retry);
    let BrowserBody::Unlistable(failure) = browser_body(&unlistable(retryable)) else {
        panic!("expected an unlistable body");
    };
    assert_eq!(failure.hint, Some("Reopening the archive may succeed."));

    // `ApplicationError::new` defaults to `Fatal` with no suggestion.
    let fatal = listing_error("the archive is corrupt");
    let BrowserBody::Unlistable(failure) = browser_body(&unlistable(fatal)) else {
        panic!("expected an unlistable body");
    };
    assert_eq!(
        failure.hint, None,
        "a fatal failure with no suggested action must not be given invented advice"
    );
}

/// A listing that failed with nothing ever answered before it.
fn unlistable(error: ApplicationError) -> RequestStatus {
    RequestStatus::Unlistable(Arc::new(error))
}

/// A listing that failed over contents already answered.
fn stale(error: ApplicationError) -> RequestStatus {
    RequestStatus::Stale(Arc::new(error))
}

// =========================================================================
// the drawn frame
// =========================================================================

/// The motivating bug's second half. A listing that failed must not draw
/// the file list at all -- an empty one is the silent-empty-view the
/// model was reshaped to prevent -- and the reason has to be on screen.
#[test]
fn a_failed_listing_draws_the_failure_and_says_it_is_not_an_empty_archive() {
    let shared = common::create_test_shared_state();
    seeded_tab(
        &shared,
        Vec::new(),
        &unlistable(
            listing_error("the archive session is gone")
                .with_suggested_action(SuggestedAction::Retry),
        ),
    );

    let harness = render(&shared);
    assert!(drew(&harness, UNLISTABLE_HEADLINE));
    assert!(
        drew(&harness, "the archive session is gone"),
        "the error's own summary must be reachable, not swallowed into a \
         generic message"
    );
    assert!(drew(&harness, "Reopening the archive may succeed."));
    assert!(
        drew(&harness, "this is not an empty archive"),
        "the panel must say what an empty file list would otherwise imply"
    );
    assert!(!drew(&harness, STALE_HEADLINE));
}

/// The complement, and the case that makes every other test in this file
/// meaningful: a folder the session listed as empty still draws as an
/// ordinary empty folder, with nothing suggesting a failure.
///
/// The positive assertion is what makes it a test rather than three ways
/// of saying "not that": `render_list_view` draws a `Name` column header
/// even over zero rows, so the file list being on screen at all is
/// observable. Without it this would pass just as happily if the central
/// panel drew nothing whatsoever.
#[test]
fn an_empty_directory_still_draws_as_an_empty_directory() {
    let shared = common::create_test_shared_state();
    seeded_tab(&shared, Vec::new(), &RequestStatus::Listed);

    let harness = render(&shared);
    assert!(
        drew(&harness, FILE_LIST_COLUMN),
        "the empty file list itself must be on screen -- an empty folder is \
         something the browser draws, not something it omits"
    );
    assert!(
        !drew(&harness, UNLISTABLE_HEADLINE),
        "an empty folder must not be reported as a failure"
    );
    assert!(!drew(&harness, STALE_HEADLINE));
    assert!(!drew(&harness, LOADING_HEADLINE));
}

/// The bug this file's model change exists for, at the drawing end.
///
/// Reopen-closed, duplicate tab, session restore and replace-active all
/// point a *fresh* tab at an archive before its open has produced
/// anything. Such a tab publishes no rows and has asked for no listing,
/// which is indistinguishable from an empty archive by every measure
/// except the one the listing now records -- so it drew as a loaded,
/// empty archive.
#[test]
fn a_tab_that_has_never_listed_does_not_draw_as_an_empty_archive() {
    let shared = common::create_test_shared_state();
    let tab = seeded_tab(&shared, Vec::new(), &RequestStatus::Unlisted);
    assert!(
        tab.archive_loaded.get(),
        "the tab names an archive, which is what makes the browser draw a \
         listing at all -- the state under test is only reachable this way"
    );

    let harness = render(&shared);
    assert!(
        !drew(&harness, FILE_LIST_COLUMN),
        "an archive nobody has listed must not draw the file list: an empty \
         one claims the archive is empty"
    );
    assert!(
        drew(&harness, LOADING_HEADLINE),
        "what it draws instead is that the contents are not known yet"
    );
    assert!(!drew(&harness, UNLISTABLE_HEADLINE));
    assert!(!drew(&harness, STALE_HEADLINE));
}

/// A failed *refresh* is the one case where rows and a failure coexist:
/// the rows are the same archive's last good answer, so they stay, and
/// the banner says why they are not a fresh one.
#[test]
fn a_failed_refresh_draws_its_rows_under_a_notice_saying_why_they_are_stale() {
    let shared = common::create_test_shared_state();
    seeded_tab(
        &shared,
        vec![row("kept.txt"), row("second.txt")],
        &stale(listing_error("the session was closed mid-refresh")),
    );

    let harness = render(&shared);
    assert!(drew(&harness, STALE_HEADLINE));
    assert!(drew(&harness, "the session was closed mid-refresh"));
    assert!(
        drew(&harness, "kept.txt") && drew(&harness, "second.txt"),
        "the rows themselves must still be drawn, not just the notice"
    );
    assert!(
        !drew(&harness, UNLISTABLE_HEADLINE),
        "rows that are still the last good answer must not be replaced by \
         the contents-unknown panel"
    );
}

/// A first listing still in flight draws as in flight -- not as an empty
/// folder, and not as a failure. This is the state a reused tab passes
/// through between being re-pointed at a new archive and that archive's
/// rows arriving, which is exactly when the previous archive's rows used
/// to be on screen under the new archive's name.
#[test]
fn a_first_listing_in_flight_draws_as_in_flight() {
    let shared = common::create_test_shared_state();
    seeded_tab(&shared, Vec::new(), &RequestStatus::Loading);

    let harness = render(&shared);
    assert!(drew(&harness, LOADING_HEADLINE));
    assert!(!drew(&harness, FILE_LIST_COLUMN));
    assert!(!drew(&harness, UNLISTABLE_HEADLINE));
    assert!(!drew(&harness, STALE_HEADLINE));
}

/// A refresh of contents already answered is the opposite: they are still
/// the last good answer, so they stay on screen with no banner -- and that
/// holds when the answer was "nothing here". Reading this off a row count
/// put a spinner over an empty folder every time a mutation refreshed it,
/// where the same mutation over a folder with rows left them alone.
#[test]
fn a_refresh_of_a_folder_answered_as_empty_keeps_drawing_the_empty_folder() {
    let shared = common::create_test_shared_state();
    seeded_tab(&shared, Vec::new(), &RequestStatus::Refreshing);

    let harness = render(&shared);
    assert!(
        drew(&harness, FILE_LIST_COLUMN),
        "the folder is known to be empty; refreshing it does not make it unknown"
    );
    assert!(!drew(&harness, LOADING_HEADLINE));
    assert!(!drew(&harness, UNLISTABLE_HEADLINE));
    assert!(!drew(&harness, STALE_HEADLINE));
}

// =========================================================================
// the side panels -- drawn from the archive, not from the folder on screen
// =========================================================================

/// While the archive's entry tree is unknown the side panels have nothing
/// truthful to draw: the tree panel's folder set and the properties
/// panel's archive totals both come from it, so they would report an
/// archive of zero files. The failure is drawn alone instead.
#[test]
fn a_failed_open_draws_no_panel_that_would_claim_the_archive_is_empty() {
    let shared = common::create_test_shared_state();
    let tab = seeded_tab(
        &shared,
        Vec::new(),
        &unlistable(listing_error("the archive session is gone")),
    );
    assert_eq!(tab.inventory.get().revision(), None);

    let harness = render(&shared);
    assert!(drew(&harness, UNLISTABLE_HEADLINE));
    assert!(
        !drew(&harness, TREE_PANEL_HEADING),
        "the tree panel must not draw a folder set derived from nothing"
    );
}

/// The same rule for a tab that has never listed at all: the archive's
/// tree is just as unknown, so the panels projected from it must not draw
/// an archive of zero files while the open behind the tab is still
/// running.
#[test]
fn a_tab_that_has_never_listed_draws_no_panel_derived_from_an_unknown_archive() {
    let shared = common::create_test_shared_state();
    let tab = seeded_tab(&shared, Vec::new(), &RequestStatus::Unlisted);
    assert_eq!(tab.inventory.get().revision(), None);

    let harness = render(&shared);
    assert!(drew(&harness, LOADING_HEADLINE));
    assert!(!drew(&harness, TREE_PANEL_HEADING));
}

/// A directory that is genuinely empty says nothing about the archive
/// around it: when a refresh of it fails, the archive's own tree is still
/// known, so the panels it feeds stay -- and the central panel says the
/// rows could not be *refreshed*, not that the contents are unknown.
///
/// The second assertion is the one that changed. Deciding between those
/// two answers by asking whether the folder had rows got this exactly
/// backwards for an empty folder: the tab knew perfectly well what was in
/// it -- nothing -- and reported that as unknown.
#[test]
fn a_failed_refresh_of_an_empty_folder_reads_as_a_failed_refresh_not_as_unknown() {
    let shared = common::create_test_shared_state();
    let tab = seeded_tab(
        &shared,
        Vec::new(),
        &stale(listing_error("the refresh failed")),
    );
    seed_inventory(&tab);

    let harness = render(&shared);
    assert!(
        drew(&harness, STALE_HEADLINE) && drew(&harness, "the refresh failed"),
        "the folder was answered as empty and the refresh of it failed; both \
         halves of that must be on screen"
    );
    assert!(
        !drew(&harness, UNLISTABLE_HEADLINE),
        "contents last answered as empty are not contents unknown"
    );
    assert!(
        drew(&harness, FILE_LIST_COLUMN),
        "the empty folder itself stays drawn under the notice"
    );
    assert!(
        drew(&harness, TREE_PANEL_HEADING),
        "the archive's folder set is still known, so the tree panel stays"
    );
}
