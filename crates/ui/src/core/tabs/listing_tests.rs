// crates/ui/src/core/tabs/listing_tests.rs
//
// Parity coverage for the navigation rules `arclain_core::archive::
// NavigationState` used to own. Each test below names the behavior it
// pins so a future change to `ArchiveNavigation` cannot quietly alter
// the breadcrumb/history UX.
//
// `NavigationState` itself is still compiled and tested in
// `arclain_core`, but nothing calls it anymore: the browser's last
// caller went with the render path's move onto the session's own rows.
// These tests do not replace its tests -- they pin that the tab's own
// cursor behaves the way that type did, which is what a rule-by-rule
// port has to prove. The original tests running alongside them is what
// let that be checked rather than asserted.

use super::*;
use arclain_app::archive::{EntrySortKey, SortDirection, ALL_ENTRIES_IN_ONE_DIRECTORY};
use arclain_app::error::{ApplicationErrorKind, Recoverability};
use arclain_app::ids::ArchiveSessionId;

/// The session every reply below is made against unless a test
/// deliberately names another one.
fn session() -> ArchiveSessionId {
    ArchiveSessionId::from_raw(1)
}

/// A listing bound to [`session`].
fn listing() -> TabListing {
    TabListing::for_session(Some(session()))
}

fn listing_error(summary: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, summary)
        .with_recoverability(Recoverability::Retry)
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
// TabListing -- the request over the navigation cursor
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
    assert_eq!(listing.status(), &RequestStatus::Unlisted);
}

#[test]
fn navigating_re_points_the_request_at_the_directory_now_browsed() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    assert_eq!(listing.directory().as_str(), "game");
    assert_eq!(listing.request().directory.as_str(), "game");
    assert_eq!(listing.current_path(), "game");
}

#[test]
fn a_refused_navigation_leaves_the_request_where_it_was() {
    let mut listing = listing();
    assert!(listing.descend("game"));

    assert!(!listing.descend(""));
    assert!(!listing.go_to("game"));
    assert_eq!(listing.directory().as_str(), "game");
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
// status -- what the fetch behind the browser's rows is doing
// =========================================================================

/// The whole reason status is its own axis. The rows live on the tab (the
/// browser rows scoped out of the inventory), so *nothing* about "there
/// are no rows" says why -- an empty directory and a listing that failed
/// look identical from the rows alone, and without this axis the second
/// renders as the first.
#[test]
fn a_failed_listing_records_the_whole_error_envelope_the_renderer_needs() {
    let mut answered = listing();
    let generation = answered.begin_loading();
    assert!(answered.succeed(generation, session(), &ArchivePath::root()));

    let mut failed = listing();
    let generation = failed.begin_loading();
    assert!(failed.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("backend exploded")
    ));

    assert_eq!(answered.status(), &RequestStatus::Listed);
    assert_eq!(answered.failure(), None);
    assert!(matches!(failed.status(), RequestStatus::Unlistable(_)));

    let error = failed
        .failure()
        .expect("the failure envelope is observable");
    assert_eq!(error.summary, "backend exploded");
    assert_eq!(
        error.recoverability,
        Recoverability::Retry,
        "the whole envelope is kept, not a summary string -- a renderer \
         needs the recoverability to say whether a retry is worth offering"
    );
}

/// A failed *refresh* records the failure without disturbing anything
/// else: the rows it stranded are the tab's, and they stay exactly as
/// they were, so a renderer can mark them stale instead of choosing
/// between showing them and reporting the error.
///
/// It lands on [`RequestStatus::Stale`] rather than
/// [`RequestStatus::Unlistable`] because a listing had already answered
/// here -- the difference between "this is the last known contents" and
/// "the contents are unknown".
#[test]
fn a_failure_after_a_successful_listing_replaces_only_the_status() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.succeed(generation, session(), &ArchivePath::root()));
    assert_eq!(listing.status(), &RequestStatus::Listed);

    let generation = listing.begin_loading();
    assert_eq!(
        listing.status(),
        &RequestStatus::Refreshing,
        "a fetch over contents already answered is a refresh, not a first listing"
    );
    assert!(
        listing.fail(
            generation,
            session(),
            &ArchivePath::root(),
            listing_error("refresh failed")
        ),
        "a failure for the directory being browsed must be recorded, not dropped"
    );

    assert!(matches!(listing.status(), RequestStatus::Stale(_)));
    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("refresh failed")
    );
    assert_eq!(
        listing.directory(),
        &ArchivePath::root(),
        "a failure must not move the cursor"
    );
}

/// The sibling of the test above, and the reason the stale/unlistable
/// split is read off the request rather than off the tab's row count.
///
/// A directory the session answered as *empty* publishes no rows, exactly
/// like an archive nothing has ever listed. Counting rows therefore
/// reported a failed refresh of an empty folder as contents-unknown --
/// strictly worse information than the truth, which is that the folder was
/// empty last time anyone looked and this attempt could not confirm it.
#[test]
fn a_failed_refresh_of_a_directory_answered_as_empty_is_still_a_failed_refresh() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    // The session answered; that the answer was "nothing here" is the
    // whole point -- no rows are published either way.
    assert!(listing.succeed(generation, session(), &ArchivePath::root()));

    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("the session went away")
    ));

    assert!(
        matches!(listing.status(), RequestStatus::Stale(_)),
        "an answered-empty directory whose refresh failed knows its last \
         contents; it must not report them as unknown: {:?}",
        listing.status()
    );
    assert!(listing.status().contents_known());
}

/// The two outcomes of a reply must be tellable apart by a caller: one
/// for the directory on screen is recorded (`true`), one for a directory
/// already navigated away from is refused (`false`) and changes nothing.
/// Both directions, because a stray success is as damaging as a stray
/// failure -- it would clear a genuine `Loading`.
#[test]
fn a_recorded_reply_and_a_refused_one_are_distinguishable() {
    let mut listing = listing();
    assert!(listing.descend("game"));
    let generation = listing.begin_loading();
    let current = listing.directory().clone();
    let left_behind = ArchivePath::root();

    assert!(
        !listing.fail(
            generation,
            session(),
            &left_behind,
            listing_error("root failed, too late")
        ),
        "a failure for a directory no longer browsed must be refused"
    );
    assert!(
        !listing.succeed(generation, session(), &left_behind),
        "a success for a directory no longer browsed must be refused"
    );
    assert!(listing.is_loading(), "a refused reply changes nothing");
    assert_eq!(listing.failure(), None);

    assert!(listing.fail(
        generation,
        session(),
        &current,
        listing_error("game failed")
    ));
    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("game failed")
    );
}

/// The distinction the `Unlisted` state exists for, stated on the model
/// itself: a listing nobody has asked anything of and a listing that
/// answered with an empty directory are different values, not the same
/// value told apart by counting rows.
///
/// This is the conflation that made a tab merely *pointed* at an archive
/// -- by reopen-closed, duplicate, session restore or replace-active --
/// draw as a loaded, empty one, and made a failed open behind such a tab
/// permanent: there was no state left for the outcome to land in.
#[test]
fn a_listing_that_has_asked_nothing_is_not_a_listing_that_answered_with_nothing() {
    let never_asked = listing();
    let mut answered = listing();
    let generation = answered.begin_loading();
    assert!(answered.succeed(generation, session(), &ArchivePath::root()));

    assert_eq!(never_asked.status(), &RequestStatus::Unlisted);
    assert_eq!(answered.status(), &RequestStatus::Listed);
    assert_ne!(
        never_asked.status(),
        answered.status(),
        "an unlisted archive and an empty one must not be the same value -- \
         neither publishes a row, so nothing else can tell them apart"
    );

    assert!(!never_asked.status().contents_known());
    assert!(answered.status().contents_known());

    // ...and it is not merely a third name for in-flight or failed.
    assert!(!never_asked.is_loading());
    assert_eq!(never_asked.failure(), None);
}

#[test]
fn a_listing_in_flight_is_neither_answered_nor_failed() {
    let mut listing = listing();
    listing.begin_loading();

    assert!(listing.is_loading());
    assert_eq!(listing.status(), &RequestStatus::Loading);
    assert!(
        !listing.status().contents_known(),
        "a first listing in flight has answered nothing yet"
    );
    assert_eq!(listing.failure(), None);
}

/// The terminal outcome that had nowhere to land. An open that fails
/// before ever minting a session cannot present the token
/// [`TabListing::fail`] demands, so without this the tab kept whatever it
/// had -- `Unlisted` with no operation left in flight, which the browser
/// can only draw as an archive still being read.
#[test]
fn an_open_that_failed_before_any_listing_settles_the_tab_that_never_listed() {
    let mut listing = listing();
    assert!(listing.fail_unlisted(listing_error("the file is not there")));

    assert!(matches!(listing.status(), RequestStatus::Unlistable(_)));
    assert_eq!(
        listing.failure().map(|error| error.summary.as_str()),
        Some("the file is not there")
    );
    assert!(!listing.is_loading(), "nothing is coming");
    assert!(
        !listing.is_unlisted(),
        "the outcome is settled, not pending"
    );
}

/// The guard that keeps a failed open off a tab that has an archive on
/// screen. Opening B into a tab showing A and having B fail says nothing
/// about A, whose rows and inventory still describe it correctly -- and a
/// listing genuinely in flight is entitled to be settled by its own reply,
/// not by an unrelated open's failure.
#[test]
fn a_failed_open_cannot_deface_a_listing_that_answered_or_one_still_running() {
    let mut answered = listing();
    let generation = answered.begin_loading();
    assert!(answered.succeed(generation, session(), &ArchivePath::root()));
    assert!(!answered.fail_unlisted(listing_error("a different archive failed")));
    assert_eq!(answered.status(), &RequestStatus::Listed);
    assert_eq!(answered.failure(), None);

    let mut in_flight = listing();
    in_flight.begin_loading();
    assert!(!in_flight.fail_unlisted(listing_error("a different archive failed")));
    assert_eq!(in_flight.status(), &RequestStatus::Loading);

    let mut already_failed = listing();
    assert!(already_failed.fail_unlisted(listing_error("the first reason")));
    assert!(
        !already_failed.fail_unlisted(listing_error("a later, unrelated reason")),
        "the recorded reason is the one that settled this tab"
    );
    assert_eq!(
        already_failed.failure().map(|error| error.summary.as_str()),
        Some("the first reason")
    );
}

#[test]
fn a_success_clears_both_the_in_flight_marker_and_an_earlier_failure() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("first attempt")
    ));
    let generation = listing.begin_loading();
    assert!(listing.succeed(generation, session(), &ArchivePath::root()));

    assert!(!listing.is_loading());
    assert_eq!(
        listing.failure(),
        None,
        "a successful listing supersedes the failure"
    );
}

#[test]
fn navigating_discards_the_status_that_described_the_previous_directory() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("root refresh failed")
    ));

    assert!(listing.descend("game"));
    assert_eq!(
        listing.status(),
        &RequestStatus::Unlisted,
        "the root failure said nothing about the folder now shown -- and \
         since the archive was never listed, neither does anything else"
    );
    assert_eq!(listing.failure(), None);
}

/// Navigation discards the *status* but not the knowledge behind it. The
/// browser answers a move by scoping the whole-archive inventory the tab
/// already holds, so a listed archive still knows what is in the folder
/// the user just walked into -- including that it is empty. Resetting to
/// `Unlisted` on every move would make each step into an empty folder
/// claim the contents are unknown.
#[test]
fn navigating_within_a_listed_archive_keeps_knowing_what_is_in_it() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.succeed(generation, session(), &ArchivePath::root()));

    assert!(listing.descend("game"));
    assert_eq!(listing.status(), &RequestStatus::Listed);
    assert!(listing.status().contents_known());

    // ...and a refresh of the folder just entered is a refresh, so a
    // failure of it strands the last known contents rather than making
    // them unknown.
    let generation = listing.begin_loading();
    assert_eq!(listing.status(), &RequestStatus::Refreshing);
    assert!(listing.fail(
        generation,
        session(),
        &ArchivePath::parse("game".to_string()).unwrap(),
        listing_error("mid-refresh")
    ));
    assert!(matches!(listing.status(), RequestStatus::Stale(_)));
}

#[test]
fn a_retry_that_also_fails_replaces_the_earlier_failure() {
    let mut listing = listing();
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("first attempt")
    ));
    let generation = listing.begin_loading();
    assert!(listing.fail(
        generation,
        session(),
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
/// *after* a newer request already answered. Without the generation guard
/// the stale failure would mark a listing the newer request just
/// refreshed as failed -- a renderer would draw "couldn't refresh" over
/// current data.
#[test]
fn a_superseded_requests_late_failure_does_not_mark_a_fresh_listing_failed() {
    let mut listing = listing();
    let first = listing.begin_loading();
    let second = listing.begin_loading();
    assert!(listing.succeed(second, session(), &ArchivePath::root()));
    assert_eq!(listing.status(), &RequestStatus::Listed);

    assert!(
        !listing.fail(
            first,
            session(),
            &ArchivePath::root(),
            listing_error("request 1, very late")
        ),
        "a superseded request's failure must be dropped, not recorded"
    );
    assert_eq!(listing.status(), &RequestStatus::Listed);
    assert_eq!(listing.failure(), None);
}

/// The mirror direction: a slow request *succeeds* after a newer request
/// already began. Without the generation guard the stale success would
/// erase the newer request's genuine `Loading`.
#[test]
fn a_superseded_requests_late_success_does_not_erase_a_newer_requests_loading() {
    let mut listing = listing();
    let first = listing.begin_loading();
    let _second = listing.begin_loading();

    assert!(
        !listing.succeed(first, session(), &ArchivePath::root()),
        "a superseded request's success must be dropped, not applied"
    );
    assert!(
        listing.is_loading(),
        "the newer request's in-flight marker must survive the late reply"
    );
}

/// The session guard, in both directions: a rebind restarts the
/// generation counter, so a numerically colliding token from the previous
/// binding must not let the old session's reply deface (or clear) the new
/// session's status.
#[test]
fn a_reply_from_another_session_is_refused_even_on_a_colliding_token() {
    let mut listing = listing();
    let stale = listing.begin_loading();

    listing = TabListing::for_session(Some(ArchiveSessionId::from_raw(2)));
    let fresh = listing.begin_loading();
    // Same numeric token value, different binding.
    assert_eq!(stale, fresh);

    assert!(
        !listing.fail(
            stale,
            session(),
            &ArchivePath::root(),
            listing_error("old session, very late")
        ),
        "a failure made against the previous session must be dropped"
    );
    assert!(
        !listing.succeed(stale, session(), &ArchivePath::root()),
        "a success made against the previous session must be dropped -- \
         otherwise it clears the new session's genuine Loading"
    );
    assert!(
        listing.is_loading(),
        "the new session's attempt is untouched"
    );
    assert_eq!(listing.failure(), None);
}

/// A tab with no archive open has no session to answer for, so no reply
/// can reach it at all.
#[test]
fn a_sessionless_listing_accepts_no_reply() {
    let mut listing = TabListing::default();
    assert_eq!(listing.session(), None);
    let generation = listing.begin_loading();

    assert!(!listing.succeed(generation, session(), &ArchivePath::root()));
    assert!(!listing.fail(
        generation,
        session(),
        &ArchivePath::root(),
        listing_error("anything")
    ));
    assert!(listing.is_loading());
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
        !listing.succeed(stale, session(), &ArchivePath::root()),
        "the round trip restored the directory, but not the request"
    );
    assert!(
        !listing.fail(
            stale,
            session(),
            &ArchivePath::root(),
            listing_error("stale failure")
        ),
        "same for the failure side"
    );
    assert_eq!(listing.status(), &RequestStatus::Unlisted);
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
