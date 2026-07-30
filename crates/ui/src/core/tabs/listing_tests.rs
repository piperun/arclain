// crates/ui/src/core/tabs/listing_tests.rs
//
// Parity coverage for the navigation rules `arclain_core::archive::
// NavigationState` used to own. Each test below names the behavior it
// pins so a future change to `ArchiveNavigation` cannot quietly alter
// the breadcrumb/history UX.
//
// `NavigationState` itself is still alive in `arclain_core`, still tested
// there, and still called by the `TRANSITIONAL(4c)` projections in
// `crate::core::operations::navigation_view` -- these tests do not
// replace its tests, they pin that the tab's own cursor behaves the same
// way. That the original tests still run is what let the rule-by-rule
// parity be checked rather than asserted; both sets go when the
// render-side migration removes the last caller.

use super::*;
use arclain_app::archive::{EntrySortKey, SortDirection, ALL_ENTRIES_IN_ONE_DIRECTORY};
use arclain_app::error::{ApplicationErrorKind, Recoverability};
use arclain_app::ids::{ArchiveSessionId, EntryId};

/// A listing bound to session 1 -- what every page helper below answers
/// for unless a test deliberately names another session.
fn listing() -> TabListing {
    TabListing::for_session(Some(ArchiveSessionId::from_raw(1)))
}

fn listing_error(summary: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, summary)
        .with_recoverability(Recoverability::Retry)
}

fn page(directory: &str, revision: u64, session: u64, names: &[&str]) -> EntryPage {
    EntryPage {
        session_id: ArchiveSessionId::from_raw(session),
        revision,
        directory: ArchivePath::parse(directory.to_string()).unwrap(),
        total: names.len() as u64,
        entries: names
            .iter()
            .enumerate()
            .map(|(index, name)| ArchiveEntryDto {
                id: EntryId::from_raw(index as u64 + 1),
                path: ArchivePath::parse(if directory.is_empty() {
                    (*name).to_string()
                } else {
                    format!("{directory}/{name}")
                })
                .unwrap(),
                name: (*name).to_string(),
                kind: arclain_app::archive::EntryKind::File,
                compressed_size: Some(0),
                uncompressed_size: 0,
                modified_at_unix_ms: None,
                encrypted: false,
                crc32: None,
            })
            .collect(),
    }
}

// =========================================================================
// ArchiveNavigation -- cursor and history
// =========================================================================

#[test]
fn a_fresh_navigation_sits_at_the_root_with_nowhere_to_go() {
    let navigation = ArchiveNavigation::default();
    assert_eq!(navigation.current_path(), "");
    assert!(!navigation.can_go_back());
    assert!(!navigation.can_go_forward());
    assert!(!navigation.can_go_up());
}

#[test]
fn descending_appends_to_the_current_directory() {
    let mut navigation = ArchiveNavigation::default();
    assert!(navigation.descend("folder1"));
    assert_eq!(navigation.current_path(), "folder1");
    assert!(navigation.can_go_back());
    assert!(navigation.can_go_up());

    assert!(navigation.descend("subfolder"));
    assert_eq!(navigation.current_path(), "folder1/subfolder");
}

#[test]
fn descending_into_an_empty_fragment_is_a_no_op() {
    let mut navigation = ArchiveNavigation::default();
    assert!(!navigation.descend(""));
    assert!(!navigation.descend("//"));
    assert!(!navigation.descend("\\"));
    assert_eq!(navigation.current_path(), "");
}

#[test]
fn descending_collapses_redundant_separators_the_way_the_old_string_cursor_did() {
    let mut navigation = ArchiveNavigation::default();
    assert!(navigation.descend("/a//b\\c/"));
    assert_eq!(navigation.current_path(), "a/b/c");
}

#[test]
fn back_walks_the_history_then_steps_out_to_the_root() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("folder1");
    navigation.descend("subfolder");

    assert!(navigation.back());
    assert_eq!(navigation.current_path(), "folder1");
    assert!(navigation.can_go_forward());

    assert!(navigation.back());
    assert_eq!(navigation.current_path(), "");
}

#[test]
fn back_at_the_root_reports_that_it_did_not_move() {
    let mut navigation = ArchiveNavigation::default();
    assert!(!navigation.back());
}

#[test]
fn forward_returns_to_what_back_walked_out_of() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("folder1");
    navigation.back();
    assert!(navigation.can_go_forward());

    assert!(navigation.forward());
    assert_eq!(navigation.current_path(), "folder1");
    assert!(!navigation.can_go_forward());
}

#[test]
fn a_fresh_navigation_clears_the_forward_history() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("a");
    navigation.back();
    assert!(navigation.can_go_forward());

    navigation.descend("b");
    assert!(!navigation.can_go_forward());
}

#[test]
fn up_climbs_one_level_per_call_and_stops_at_the_root() {
    let mut navigation = ArchiveNavigation::default();
    assert!(navigation.go_to("a/b/c"));

    assert!(navigation.up());
    assert_eq!(navigation.current_path(), "a/b");

    assert!(navigation.up());
    assert_eq!(navigation.current_path(), "a");

    assert!(navigation.up());
    assert_eq!(navigation.current_path(), "");

    assert!(!navigation.up());
}

#[test]
fn go_to_replaces_the_whole_path_rather_than_appending() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("folder1");
    assert!(navigation.go_to("other/deep"));
    assert_eq!(navigation.current_path(), "other/deep");
}

#[test]
fn go_to_the_directory_already_shown_leaves_the_history_untouched() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("folder1");
    let before = navigation.clone();

    assert!(!navigation.go_to("folder1"));
    assert_eq!(navigation, before);
}

/// The one deliberate behavior change from `NavigationState`: a target
/// that is not a legal archive-relative path is refused outright. The
/// old string cursor accepted it and then simply matched no entries,
/// which meant a crafted breadcrumb could become the `directory` of a
/// listing request.
#[test]
fn a_traversing_or_absolute_target_is_refused_rather_than_navigated_to() {
    let mut navigation = ArchiveNavigation::default();
    navigation.descend("folder1");

    assert!(!navigation.descend(".."));
    assert!(!navigation.descend("../escape"));
    assert!(!navigation.go_to("../../etc"));
    assert!(!navigation.go_to("C:/Windows"));
    assert_eq!(navigation.current_path(), "folder1");
}

/// A leading `/` is a redundant separator, not an absolute path: the
/// fragment normalizer drops it, exactly as the old string cursor did,
/// so a breadcrumb that hands back `/game` still navigates to `game`.
#[test]
fn a_leading_separator_is_normalized_away_not_treated_as_absolute() {
    let mut navigation = ArchiveNavigation::default();
    assert!(navigation.go_to("/game/data"));
    assert_eq!(navigation.current_path(), "game/data");
}

// =========================================================================
// TabListing -- request/page bookkeeping over the navigation cursor
// =========================================================================

#[test]
fn a_fresh_listing_requests_the_whole_root_directory_by_name() {
    let listing = listing();
    assert_eq!(listing.directory(), &ArchivePath::root());
    assert_eq!(listing.request().sort_key, EntrySortKey::Name);
    assert_eq!(listing.request().sort_direction, SortDirection::Ascending);
    assert_eq!(listing.request().name_filter, None);
    assert_eq!(listing.request().offset, 0);
    assert_eq!(listing.request().limit, ALL_ENTRIES_IN_ONE_DIRECTORY);
    assert!(listing.page().is_none());
    assert!(listing.entries().is_empty());
}

#[test]
fn navigating_re_points_the_request_and_drops_the_page_it_no_longer_answers() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 1, &["readme.txt"])));
    assert_eq!(listing.entries().len(), 1);

    assert!(listing.descend("game"));
    assert_eq!(listing.directory().as_str(), "game");
    assert!(
        listing.page().is_none(),
        "the root page must not be shown as the contents of game/"
    );
    assert!(listing.entries().is_empty());
}

#[test]
fn a_refused_navigation_leaves_the_request_and_page_alone() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("game", 1, 1, &["Game.exe"])));

    assert!(!listing.descend(""));
    assert!(!listing.go_to("game"));
    assert_eq!(listing.directory().as_str(), "game");
    assert_eq!(listing.entries().len(), 1);
}

#[test]
fn a_page_for_a_different_directory_is_refused() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    let generation = listing.begin_loading();

    assert!(
        !listing.adopt_page(generation, page("", 1, 1, &["readme.txt"])),
        "a root listing landing late must not be shown as game/'s contents"
    );
    assert!(listing.page().is_none());
}

#[test]
fn a_page_older_than_the_one_held_for_the_same_session_is_refused() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 5, 1, &["current.txt"])));

    let generation = listing.begin_loading();
    assert!(!listing.adopt_page(generation, page("", 4, 1, &["stale.txt"])));
    assert_eq!(listing.entries()[0].name, "current.txt");

    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 5, 1, &["same-revision-refresh.txt"])));
    assert_eq!(listing.entries()[0].name, "same-revision-refresh.txt");

    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 6, 1, &["newer.txt"])));
    assert_eq!(listing.entries()[0].name, "newer.txt");
}

/// The guard that matters most. `EntryId` is unique only *within* its
/// session, so a page from the archive a tab held before this one does
/// not merely show the wrong rows -- its ids can name entirely different
/// entries in the session the tab holds now, and an extract or a delete
/// is addressed by exactly those ids.
#[test]
fn a_page_from_another_session_is_refused_however_new_it_looks() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 1, &["this-archive.txt"])));

    let generation = listing.begin_loading();
    assert!(!listing.adopt_page(generation, page("", 99, 2, &["other-archive.txt"])));
    assert_eq!(listing.entries()[0].name, "this-archive.txt");
}

/// A tab with no archive open has no session to answer for, so nothing
/// can be seated into it at all.
#[test]
fn a_sessionless_listing_adopts_nothing() {
    let mut listing = TabListing::default();
    assert_eq!(listing.session(), None);
    let generation = listing.begin_loading();
    assert!(!listing.adopt_page(generation, page("", 1, 1, &["anything.txt"])));
    assert!(listing.page().is_none());
}

/// Reopening an archive into the tab rebinds the listing, and only then
/// does the new session's page seat -- a fresh session starts at
/// revision 1, so the revision guard must not outlive the rebind.
#[test]
fn rebinding_to_a_new_session_lets_its_first_page_seat_at_revision_one() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 7, 1, &["old-archive.txt"])));

    listing = TabListing::for_session(Some(ArchiveSessionId::from_raw(2)));
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 2, &["new-archive.txt"])));
    assert_eq!(listing.entries()[0].name, "new-archive.txt");
}

#[test]
fn navigating_resets_the_page_window_to_the_start_of_the_new_directory() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    assert_eq!(listing.request().offset, 0);
    assert_eq!(listing.request().directory.as_str(), "game");

    assert!(listing.up());
    assert_eq!(listing.request().offset, 0);
    assert_eq!(listing.request().directory, ArchivePath::root());
}

#[test]
fn the_listing_exposes_the_same_history_predicates_the_toolbar_reads() {
    let mut listing = listing();
    assert!(!listing.can_go_back());
    assert!(!listing.can_go_forward());
    assert!(!listing.can_go_up());

    assert!(listing.descend("game"));
    assert!(listing.can_go_back());
    assert!(listing.can_go_up());
    assert!(!listing.can_go_forward());

    assert!(listing.back());
    assert!(listing.can_go_forward());
    assert!(listing.forward());
    assert_eq!(listing.current_path(), "game");
}

// =========================================================================
// rows x status -- why a directory has no rows, and what is happening to it
// =========================================================================

/// The whole reason the two axes are separate. An `Option<EntryPage>` alone
/// gives one representation of "no rows" for every cause, and once the
/// render path reads this model, a listing that *failed* would render as a
/// perfectly ordinary empty folder.
#[test]
fn a_failed_listing_is_distinguishable_from_an_empty_directory() {
    let mut empty = listing();
    let generation = empty.begin_loading();
    assert!(empty.adopt_page(generation, page("", 1, 1, &[])));

    let mut failed = listing();
    let generation = failed.begin_loading();
    assert!(failed.fail(
        generation,
        &ArchivePath::root(),
        listing_error("backend exploded")
    ));

    // Indistinguishable on the transitional accessor, by design -- that is
    // what keeps un-migrated consumers behaving exactly as before.
    assert!(empty.entries().is_empty());
    assert!(failed.entries().is_empty());

    // Distinguishable everywhere it matters.
    assert!(
        empty.page().is_some(),
        "the session said this folder is empty"
    );
    assert!(
        failed.page().is_none(),
        "nothing is known about this folder"
    );
    assert_eq!(empty.status(), &RequestStatus::Idle);
    assert!(matches!(failed.status(), RequestStatus::Failed(_)));
    assert_eq!(empty.failure(), None);
    assert_eq!(
        failed.failure().map(|error| error.summary.as_str()),
        Some("backend exploded")
    );
}

/// The state a collapsed enum could not name: rows on screen that are the
/// last good answer to a request which has since failed. Both facts have to
/// be observable at once, or a renderer cannot mark the rows stale -- it
/// would have to choose between showing them and reporting the error.
#[test]
fn stale_rows_and_the_failure_that_stranded_them_are_both_observable() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 3, 1, &["readme.txt"])));

    let generation = listing.begin_loading();
    assert!(
        listing.fail(
            generation,
            &ArchivePath::root(),
            listing_error("refresh failed")
        ),
        "a failure for the directory being browsed must be recorded, not dropped"
    );

    assert_eq!(listing.entries()[0].name, "readme.txt");
    assert!(listing.page().is_some());
    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("refresh failed"),
        "keeping the rows must not lose the reason they are stale"
    );
}

/// The other state a collapsed enum could not name: a refresh in flight
/// over rows still on screen, so a renderer can show a spinner *without*
/// blanking the list first.
#[test]
fn a_refresh_in_flight_can_keep_the_previous_rows_on_screen() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 1, &["readme.txt"])));

    listing.begin_loading();

    assert!(listing.is_loading());
    assert_eq!(
        listing.entries()[0].name,
        "readme.txt",
        "beginning a refresh must not blank the rows on its own"
    );
    assert!(listing.page().is_some());
}

/// The two outcomes of `fail` must be tellable apart by a caller: a failure
/// for the directory on screen is recorded (`true`), one for a directory
/// already navigated away from is refused (`false`) and changes nothing.
#[test]
fn a_recorded_failure_and_a_refused_one_are_distinguishable() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    let generation = listing.begin_loading();
    let current = listing.directory().clone();
    let left_behind = ArchivePath::root();

    assert!(
        !listing.fail(
            generation,
            &left_behind,
            listing_error("root failed, too late")
        ),
        "a failure for a directory no longer browsed must be refused"
    );
    assert!(listing.is_loading(), "a refused failure changes nothing");
    assert_eq!(listing.failure(), None);

    assert!(listing.fail(generation, &current, listing_error("game failed")));
    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("game failed")
    );
}

#[test]
fn a_listing_that_has_asked_nothing_is_neither_loading_nor_failed() {
    let listing = listing();
    assert_eq!(listing.status(), &RequestStatus::Idle);
    assert!(!listing.is_loading());
    assert_eq!(listing.failure(), None);
    assert!(listing.page().is_none());
}

#[test]
fn a_first_listing_in_flight_is_neither_empty_nor_failed() {
    let mut listing = listing();
    listing.begin_loading();

    assert!(listing.is_loading());
    assert!(listing.page().is_none());
    assert_eq!(listing.failure(), None);
    assert!(listing.entries().is_empty());
}

#[test]
fn a_page_arriving_clears_both_the_in_flight_marker_and_an_earlier_failure() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        &ArchivePath::root(),
        listing_error("first attempt")
    ));
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 1, &["readme.txt"])));

    assert!(!listing.is_loading());
    assert_eq!(
        listing.failure(),
        None,
        "a successful listing supersedes the failure"
    );
    assert_eq!(listing.entries()[0].name, "readme.txt");
}

#[test]
fn navigating_discards_the_rows_and_the_status_together() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.adopt_page(generation, page("", 1, 1, &["readme.txt"])));
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        &ArchivePath::root(),
        listing_error("root refresh failed")
    ));

    assert!(listing.descend("game"));
    assert!(
        listing.page().is_none(),
        "the root rows are not the contents of the folder now shown"
    );
    assert_eq!(
        listing.status(),
        &RequestStatus::Idle,
        "the root failure said nothing about the folder now shown"
    );
    assert_eq!(listing.failure(), None);
}

#[test]
fn a_retry_that_also_fails_replaces_the_earlier_failure() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        &ArchivePath::root(),
        listing_error("first attempt")
    ));
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        &ArchivePath::root(),
        listing_error("second attempt")
    ));

    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("second attempt")
    );
}

// =========================================================================
// request identity -- late replies of superseded requests are dropped
// =========================================================================

/// One direction of the late-reply ordering hole: a slow request fails
/// *after* a newer request already refreshed the rows. Without the
/// generation guard the stale failure would mark freshly refreshed rows
/// as failed -- a renderer would draw "couldn't refresh" over current
/// data.
#[test]
fn a_superseded_requests_late_failure_does_not_mark_fresh_rows_failed() {
    let mut listing = listing();
    let first = listing.begin_loading();
    let second = listing.begin_loading();
    assert!(listing.adopt_page(second, page("", 9, 1, &["fresh.txt"])));
    assert_eq!(listing.status(), &RequestStatus::Idle);

    assert!(
        !listing.fail(
            first,
            &ArchivePath::root(),
            listing_error("request 1, very late")
        ),
        "a superseded request's failure must be dropped, not recorded"
    );
    assert_eq!(listing.status(), &RequestStatus::Idle);
    assert_eq!(listing.failure(), None);
    assert_eq!(listing.entries()[0].name, "fresh.txt");
}

/// The mirror direction: a slow request *succeeds* after a newer request
/// already began. Without the generation guard the stale success would
/// seat its rows and erase the newer request's genuine `Loading`.
#[test]
fn a_superseded_requests_late_success_does_not_erase_a_newer_requests_loading() {
    let mut listing = listing();
    let first = listing.begin_loading();
    let _second = listing.begin_loading();

    assert!(
        !listing.adopt_page(first, page("", 1, 1, &["stale.txt"])),
        "a superseded request's success must be dropped, not seated"
    );
    assert!(
        listing.is_loading(),
        "the newer request's in-flight marker must survive the late reply"
    );
    assert!(listing.page().is_none());
}

/// Navigating supersedes whatever was in flight -- including when the
/// user navigates away and back, which restores the *directory* the
/// stale reply answers. The directory guard alone cannot catch that
/// case; the generation does.
#[test]
fn navigating_away_and_back_still_drops_the_in_flight_replies_it_left_behind() {
    let mut listing = listing();
    let stale = listing.begin_loading();
    assert!(listing.descend("game"));
    assert!(listing.back());
    assert_eq!(listing.directory(), &ArchivePath::root());

    assert!(
        !listing.adopt_page(stale, page("", 1, 1, &["stale.txt"])),
        "the round trip restored the directory, but not the request"
    );
    assert!(
        !listing.fail(stale, &ArchivePath::root(), listing_error("stale failure")),
        "same for the failure side"
    );
    assert!(listing.page().is_none());
    assert_eq!(listing.status(), &RequestStatus::Idle);
}

// =========================================================================
// the whole-directory request shape (now contract-owned)
// =========================================================================

/// Regression guard for a silent data loss: extraction resolved its
/// selected paths to `EntryId`s through a listing capped at a literal
/// `100_000`, so every row selected past that in a larger directory was
/// simply not found and not extracted -- with no error, because "none of
/// the selected items are in this listing" is indistinguishable from a
/// stale selection.
#[test]
fn a_whole_directory_request_does_not_cap_below_any_directory_an_archive_can_hold() {
    let request = ListEntriesRequest::whole_directory(ArchivePath::root());

    assert_eq!(
        request.offset, 0,
        "a resolution pass must start at the first row"
    );
    assert_eq!(request.limit, ALL_ENTRIES_IN_ONE_DIRECTORY);
    assert!(
        u64::from(request.limit) > 100_000,
        "extraction once capped this at 100 000 and silently dropped every \
         selected row past that"
    );
}

/// A fresh listing asks for exactly the whole-directory shape, so the
/// browser's own request and the selection-resolution requests cannot
/// drift apart.
#[test]
fn a_fresh_listing_asks_for_the_whole_directory_shape() {
    assert_eq!(
        listing().request(),
        &ListEntriesRequest::whole_directory(ArchivePath::root())
    );
}
