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
/// `status` the caller wants the listing's fetch to be in.
///
/// The status is driven through the real seams -- `begin_loading` then
/// `succeed`/`fail` -- rather than constructed, so a test can only
/// express states the model can actually reach.
fn seeded_tab(shared: &SharedState, rows: Vec<FileEntry>, status: &RequestStatus) -> Arc<TabState> {
    let tab = shared.signals().tabs.get().active().clone();
    tab.archive_path.set(Some(PathBuf::from("fixture.zip")));

    let session_id = ArchiveSessionId::from_raw(SESSION);
    let mut listing = TabListing::for_session(Some(session_id));
    match status {
        RequestStatus::Idle => {
            let generation = listing.begin_loading();
            assert!(listing.succeed(generation, session_id, &ArchivePath::root()));
        }
        RequestStatus::Loading => {
            listing.begin_loading();
        }
        RequestStatus::Failed(error) => {
            let generation = listing.begin_loading();
            assert!(listing.fail(
                generation,
                session_id,
                &ArchivePath::root(),
                (**error).clone()
            ));
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

/// The distinction the whole two-axis model exists for. Zero rows means
/// two entirely different things depending on the status, and only one of
/// them is "this folder is empty".
#[test]
fn no_rows_means_empty_only_when_the_listing_actually_answered() {
    assert_eq!(
        browser_body(false, &RequestStatus::Idle),
        BrowserBody::Listing,
        "the session answered with nothing: the folder really is empty"
    );
    assert_eq!(
        browser_body(false, &RequestStatus::Loading),
        BrowserBody::Loading,
        "nothing has answered yet -- an empty folder would be a guess"
    );
    assert!(
        matches!(
            browser_body(false, &failed_status(listing_error("the session is gone"))),
            BrowserBody::Unlistable(_)
        ),
        "the listing failed -- the contents are unknown, not empty"
    );
}

/// Rows on screen are drawn in every state; what changes is whether a
/// reason for distrusting them is drawn alongside.
#[test]
fn rows_are_always_drawn_and_only_a_failure_marks_them_stale() {
    assert_eq!(
        browser_body(true, &RequestStatus::Idle),
        BrowserBody::Listing
    );
    assert_eq!(
        browser_body(true, &RequestStatus::Loading),
        BrowserBody::Listing,
        "a refresh in flight over existing rows draws no banner: they are \
         still the last good answer, and a banner per mutation is noise"
    );
    assert!(matches!(
        browser_body(true, &failed_status(listing_error("refresh failed"))),
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
    let BrowserBody::Unlistable(failure) = browser_body(false, &failed_status(error)) else {
        panic!("a failed listing with no rows must draw as unlistable");
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
    let BrowserBody::Unlistable(failure) = browser_body(false, &failed_status(retryable)) else {
        panic!("expected an unlistable body");
    };
    assert_eq!(failure.hint, Some("Reopening the archive may succeed."));

    // `ApplicationError::new` defaults to `Fatal` with no suggestion.
    let fatal = listing_error("the archive is corrupt");
    let BrowserBody::Unlistable(failure) = browser_body(false, &failed_status(fatal)) else {
        panic!("expected an unlistable body");
    };
    assert_eq!(
        failure.hint, None,
        "a fatal failure with no suggested action must not be given invented advice"
    );
}

fn failed_status(error: ApplicationError) -> RequestStatus {
    RequestStatus::Failed(Arc::new(error))
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
        &failed_status(
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

/// The complement, and the case that makes the one above meaningful: a
/// folder the session listed as empty still draws as an ordinary empty
/// folder, with nothing suggesting a failure.
#[test]
fn an_empty_directory_still_draws_as_an_empty_directory() {
    let shared = common::create_test_shared_state();
    seeded_tab(&shared, Vec::new(), &RequestStatus::Idle);

    let harness = render(&shared);
    assert!(
        !drew(&harness, UNLISTABLE_HEADLINE),
        "an empty folder must not be reported as a failure"
    );
    assert!(!drew(&harness, STALE_HEADLINE));
    assert!(!drew(&harness, LOADING_HEADLINE));
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
        &failed_status(listing_error("the session was closed mid-refresh")),
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

/// A listing still in flight over no rows draws as in flight -- not as an
/// empty folder, and not as a failure. This is the state a reused tab
/// passes through between being re-pointed at a new archive and that
/// archive's rows arriving, which is exactly when the previous archive's
/// rows used to be on screen under the new archive's name.
#[test]
fn a_listing_in_flight_over_no_rows_draws_as_in_flight() {
    let shared = common::create_test_shared_state();
    seeded_tab(&shared, Vec::new(), &RequestStatus::Loading);

    let harness = render(&shared);
    assert!(drew(&harness, LOADING_HEADLINE));
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
        &failed_status(listing_error("the archive session is gone")),
    );
    assert_eq!(tab.inventory.get().revision(), None);

    let harness = render(&shared);
    assert!(drew(&harness, UNLISTABLE_HEADLINE));
    assert!(
        !drew(&harness, TREE_PANEL_HEADING),
        "the tree panel must not draw a folder set derived from nothing"
    );
}

/// The complement, and the reason the rule above is keyed on the archive
/// rather than on the folder having rows. A directory that is genuinely
/// empty says nothing about the archive around it: when a refresh of it
/// fails, the archive's own tree is still known, so the panels it feeds
/// stay -- only the central panel reports the failure.
#[test]
fn a_failure_over_an_empty_folder_keeps_the_panels_the_archive_still_feeds() {
    let shared = common::create_test_shared_state();
    let tab = seeded_tab(
        &shared,
        Vec::new(),
        &failed_status(listing_error("the refresh failed")),
    );
    seed_inventory(&tab);

    let harness = render(&shared);
    assert!(drew(&harness, UNLISTABLE_HEADLINE));
    assert!(
        drew(&harness, TREE_PANEL_HEADING),
        "the archive's folder set is still known, so the tree panel stays"
    );
}
