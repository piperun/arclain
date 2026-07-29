// crates/ui/src/core/tabs/listing_tests.rs
//
// Parity coverage for the navigation rules `arclain_core::archive::
// NavigationState` used to own. Each test below names the behavior it
// pins so a future change to `ArchiveNavigation` cannot quietly alter
// the breadcrumb/history UX -- the pre-facade type's own tests are gone
// with it, and these are what replaced them.

use super::*;
use arclain_app::ids::{ArchiveSessionId, EntryId};

/// A listing bound to session 1 -- what every page helper below answers
/// for unless a test deliberately names another session.
fn listing() -> TabListing {
    TabListing::for_session(Some(ArchiveSessionId::from_raw(1)))
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
    assert!(listing.adopt_page(page("", 1, 1, &["readme.txt"])));
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
    assert!(listing.adopt_page(page("game", 1, 1, &["Game.exe"])));

    assert!(!listing.descend(""));
    assert!(!listing.go_to("game"));
    assert_eq!(listing.directory().as_str(), "game");
    assert_eq!(listing.entries().len(), 1);
}

#[test]
fn a_page_for_a_different_directory_is_refused() {
    let mut listing = listing();
    assert!(listing.descend("game"));

    assert!(
        !listing.adopt_page(page("", 1, 1, &["readme.txt"])),
        "a root listing landing late must not be shown as game/'s contents"
    );
    assert!(listing.page().is_none());
}

#[test]
fn a_page_older_than_the_one_held_for_the_same_session_is_refused() {
    let mut listing = listing();
    assert!(listing.adopt_page(page("", 5, 1, &["current.txt"])));

    assert!(!listing.adopt_page(page("", 4, 1, &["stale.txt"])));
    assert_eq!(listing.entries()[0].name, "current.txt");

    assert!(listing.adopt_page(page("", 5, 1, &["same-revision-refresh.txt"])));
    assert_eq!(listing.entries()[0].name, "same-revision-refresh.txt");

    assert!(listing.adopt_page(page("", 6, 1, &["newer.txt"])));
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
    assert!(listing.adopt_page(page("", 1, 1, &["this-archive.txt"])));

    assert!(!listing.adopt_page(page("", 99, 2, &["other-archive.txt"])));
    assert_eq!(listing.entries()[0].name, "this-archive.txt");
}

/// A tab with no archive open has no session to answer for, so nothing
/// can be seated into it at all.
#[test]
fn a_sessionless_listing_adopts_nothing() {
    let mut listing = TabListing::default();
    assert_eq!(listing.session(), None);
    assert!(!listing.adopt_page(page("", 1, 1, &["anything.txt"])));
    assert!(listing.page().is_none());
}

/// Reopening an archive into the tab rebinds the listing, and only then
/// does the new session's page seat -- a fresh session starts at
/// revision 1, so the revision guard must not outlive the rebind.
#[test]
fn rebinding_to_a_new_session_lets_its_first_page_seat_at_revision_one() {
    let mut listing = listing();
    assert!(listing.adopt_page(page("", 7, 1, &["old-archive.txt"])));

    listing = TabListing::for_session(Some(ArchiveSessionId::from_raw(2)));
    assert!(listing.adopt_page(page("", 1, 2, &["new-archive.txt"])));
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
