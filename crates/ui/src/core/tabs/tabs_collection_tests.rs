//! Tests for TabsCollection. Loaded via `#[path]` so `super::*` is the
//! contents of tabs_collection.rs.

use super::*;
use std::sync::atomic::Ordering;

#[test]
fn open_starts_with_one_tab_active() {
    let col = TabsCollection::new();
    assert_eq!(col.tabs().len(), 1);
    assert_eq!(col.active_id(), TabId(1));
}

#[test]
fn open_appends_and_activates_new_tab() {
    let mut col = TabsCollection::new();
    let new_id = col.open(Some(PathBuf::from("/tmp/a.zip")));
    assert_eq!(col.tabs().len(), 2);
    assert_eq!(col.active_id(), new_id);
    assert_ne!(new_id, TabId(1));
}

#[test]
fn open_with_no_path_creates_empty_tab() {
    let mut col = TabsCollection::new();
    let id = col.open(None);
    let tab = col.get(id).unwrap();
    assert!(tab.archive_path.get().is_none());
}

#[test]
fn close_removes_tab_from_list_when_zero_in_flight() {
    let mut col = TabsCollection::new();
    let new_id = col.open(Some(PathBuf::from("/tmp/b.zip")));
    let result = col.close(new_id);
    assert_eq!(result, CloseResult::Closed);
    assert_eq!(col.tabs().len(), 1);
}

#[test]
fn close_blocks_when_in_flight_ops_present() {
    let mut col = TabsCollection::new();
    let new_id = col.open(Some(PathBuf::from("/tmp/c.zip")));
    col.get(new_id)
        .unwrap()
        .in_flight_ops
        .store(2, Ordering::SeqCst);
    let result = col.close(new_id);
    assert_eq!(result, CloseResult::BlockedByInFlight { count: 2 });
    // Tab still present — caller decides whether to force-close.
    assert_eq!(col.tabs().len(), 2);
}

#[test]
fn close_unknown_id_returns_not_found() {
    let mut col = TabsCollection::new();
    let result = col.close(TabId(9999));
    assert_eq!(result, CloseResult::NotFound);
    assert_eq!(col.tabs().len(), 1, "no tabs should be removed");
}

#[test]
fn force_close_bypasses_in_flight_check() {
    let mut col = TabsCollection::new();
    let new_id = col.open(Some(PathBuf::from("/tmp/d.zip")));
    col.get(new_id)
        .unwrap()
        .in_flight_ops
        .store(5, Ordering::SeqCst);
    col.force_close(new_id);
    assert_eq!(col.tabs().len(), 1);
    assert!(col.get(new_id).is_none());
}

#[test]
fn close_last_tab_spawns_replacement_empty_tab() {
    let mut col = TabsCollection::new();
    let only_id = col.active_id();
    col.close(only_id);
    // Replacement spawned; never zero tabs.
    assert_eq!(col.tabs().len(), 1);
    assert_ne!(col.active_id(), only_id);
    assert!(col.active().archive_path.get().is_none());
}

#[test]
fn close_active_tab_activates_neighbour() {
    let mut col = TabsCollection::new();
    let second = col.open(Some(PathBuf::from("/tmp/e.zip")));
    let third = col.open(Some(PathBuf::from("/tmp/f.zip")));
    assert_eq!(col.active_id(), third);
    col.close(third);
    // Active falls back to the one at the same index (second after removal).
    assert_eq!(col.active_id(), second);
}

#[test]
fn replace_active_keeps_id_swaps_state() {
    let mut col = TabsCollection::new();
    let original_id = col.active_id();
    col.replace_active(PathBuf::from("/tmp/g.zip"));
    assert_eq!(col.active_id(), original_id);
    assert_eq!(
        col.active().archive_path.get(),
        Some(PathBuf::from("/tmp/g.zip"))
    );
}

#[test]
fn switch_to_changes_active() {
    let mut col = TabsCollection::new();
    let first = col.active_id();
    let second = col.open(Some(PathBuf::from("/tmp/h.zip")));
    assert_eq!(col.active_id(), second);
    col.switch_to(first);
    assert_eq!(col.active_id(), first);
}

#[test]
fn switch_to_invalid_id_is_noop() {
    let mut col = TabsCollection::new();
    let original = col.active_id();
    col.switch_to(TabId(9999));
    assert_eq!(col.active_id(), original);
}

#[test]
fn reorder_moves_tab_to_new_position() {
    let mut col = TabsCollection::new();
    let _b = col.open(Some(PathBuf::from("/tmp/i.zip")));
    let _c = col.open(Some(PathBuf::from("/tmp/j.zip")));
    let id_before_at_0 = col.tabs()[0].id;
    let id_before_at_2 = col.tabs()[2].id;
    col.reorder(0, 2);
    // Tab that was at index 0 now at index 2; old index-2 tab still present.
    assert_eq!(col.tabs()[2].id, id_before_at_0);
    assert_ne!(col.tabs()[0].id, id_before_at_0);
    assert_eq!(col.tabs().iter().filter(|t| t.id == id_before_at_2).count(), 1);
}

#[test]
fn force_close_fires_tab_cancel_flag() {
    let mut col = TabsCollection::new();
    let new_id = col.open(Some(PathBuf::from("/tmp/x.zip")));
    // Capture an Arc<TabState> clone before close — simulates a
    // background op that captured the Arc at spawn.
    let tab_arc = col.get(new_id).unwrap().clone();
    assert!(!tab_arc.tab_cancel.load(Ordering::SeqCst));

    col.force_close(new_id);

    // Tab removed from collection but the cancel flag is observable
    // on the captured Arc clone (matches the ACID isolation contract:
    // background ops can see the cancel even after the collection
    // removed the tab).
    assert!(tab_arc.tab_cancel.load(Ordering::SeqCst));
    assert!(col.get(new_id).is_none());
}

#[test]
fn next_id_is_monotonic_no_reuse() {
    let mut col = TabsCollection::new();
    let a = col.open(None);
    col.close(a);
    let b = col.open(None);
    assert_ne!(a, b, "TabId must never be reused within a session");
}

#[test]
fn reopen_last_closed_resurrects_archive_path() {
    let mut col = TabsCollection::new();
    let path = PathBuf::from("/tmp/closed.zip");
    let id = col.open(Some(path.clone()));
    col.close(id);
    assert!(col.has_recently_closed());

    let (new_id, restored_path) = col.reopen_last_closed().expect("buffer has entry");
    assert_eq!(restored_path, path);
    assert_eq!(col.get(new_id).unwrap().archive_path.get(), Some(path));
    assert!(!col.has_recently_closed(), "buffer drained after reopen");
}

#[test]
fn reopen_last_closed_returns_none_when_empty() {
    let mut col = TabsCollection::new();
    assert!(col.reopen_last_closed().is_none());
}

#[test]
fn close_empty_tab_does_not_populate_recently_closed() {
    let mut col = TabsCollection::new();
    let empty_id = col.open(None);
    col.close(empty_id);
    assert!(!col.has_recently_closed(), "empty tab should not be remembered");
}

#[test]
fn recently_closed_is_lifo() {
    let mut col = TabsCollection::new();
    let a = col.open(Some(PathBuf::from("/tmp/a.zip")));
    let b = col.open(Some(PathBuf::from("/tmp/b.zip")));
    col.close(a);
    col.close(b);
    let (_, restored) = col.reopen_last_closed().unwrap();
    assert_eq!(restored, PathBuf::from("/tmp/b.zip"), "most recent first");
    let (_, restored) = col.reopen_last_closed().unwrap();
    assert_eq!(restored, PathBuf::from("/tmp/a.zip"));
}

#[test]
fn recently_closed_ring_buffer_drops_oldest() {
    let mut col = TabsCollection::new();
    // Open and close 12 archives; only the last 10 should be reachable.
    for n in 0..12 {
        let id = col.open(Some(PathBuf::from(format!("/tmp/{}.zip", n))));
        col.close(id);
    }
    // 10 entries kept (0 and 1 dropped).
    let mut restored = Vec::new();
    while let Some((_, p)) = col.reopen_last_closed() {
        restored.push(p);
    }
    assert_eq!(restored.len(), 10);
    // Most recent first: 11, 10, ..., 2
    assert_eq!(restored.first().unwrap(), &PathBuf::from("/tmp/11.zip"));
    assert_eq!(restored.last().unwrap(), &PathBuf::from("/tmp/2.zip"));
}

#[test]
fn force_close_also_populates_recently_closed() {
    let mut col = TabsCollection::new();
    let id = col.open(Some(PathBuf::from("/tmp/forced.zip")));
    col.force_close(id);
    let (_, p) = col.reopen_last_closed().unwrap();
    assert_eq!(p, PathBuf::from("/tmp/forced.zip"));
}

#[test]
fn close_others_keeps_only_anchor() {
    let mut col = TabsCollection::new();
    let _a = col.open(Some(PathBuf::from("/tmp/a.zip")));
    let keep = col.open(Some(PathBuf::from("/tmp/keep.zip")));
    let _c = col.open(Some(PathBuf::from("/tmp/c.zip")));
    let skipped = col.close_others(keep);
    assert_eq!(skipped, 0);
    assert_eq!(col.tabs().len(), 1);
    assert_eq!(col.tabs()[0].id, keep);
}

#[test]
fn close_others_skips_in_flight_tabs() {
    let mut col = TabsCollection::new();
    let busy = col.open(Some(PathBuf::from("/tmp/busy.zip")));
    let keep = col.open(Some(PathBuf::from("/tmp/keep.zip")));
    let _idle = col.open(Some(PathBuf::from("/tmp/idle.zip")));
    col.get(busy).unwrap().in_flight_ops.store(1, Ordering::SeqCst);
    let skipped = col.close_others(keep);
    assert_eq!(skipped, 1);
    // Busy tab stays alive, idle was closed, keep stayed.
    assert!(col.get(busy).is_some());
    assert!(col.get(keep).is_some());
    assert_eq!(col.tabs().len(), 2);
}

#[test]
fn close_to_right_only_affects_later_tabs() {
    let mut col = TabsCollection::new();
    let first = TabId(1);
    let anchor = col.open(Some(PathBuf::from("/tmp/anchor.zip")));
    let _right = col.open(Some(PathBuf::from("/tmp/right.zip")));
    let _further = col.open(Some(PathBuf::from("/tmp/further.zip")));
    let skipped = col.close_to_right(anchor);
    assert_eq!(skipped, 0);
    assert_eq!(col.tabs().len(), 2);
    // Anchor and the original first tab survive.
    assert!(col.get(anchor).is_some());
    assert!(col.get(first).is_some());
}

#[test]
fn close_to_right_unknown_anchor_is_noop() {
    let mut col = TabsCollection::new();
    col.open(Some(PathBuf::from("/tmp/a.zip")));
    let before = col.tabs().len();
    let skipped = col.close_to_right(TabId(99999));
    assert_eq!(skipped, 0);
    assert_eq!(col.tabs().len(), before);
}

#[test]
fn duplicate_creates_new_tab_with_same_path() {
    let mut col = TabsCollection::new();
    let source = col.open(Some(PathBuf::from("/tmp/dup.zip")));
    let (new_id, path) = col.duplicate(source).unwrap();
    assert_ne!(new_id, source);
    assert_eq!(path, PathBuf::from("/tmp/dup.zip"));
    assert_eq!(col.get(new_id).unwrap().archive_path.get(), Some(path));
}

#[test]
fn duplicate_empty_tab_returns_none() {
    let mut col = TabsCollection::new();
    let empty = col.open(None);
    assert!(col.duplicate(empty).is_none());
}

#[test]
fn set_pinned_moves_tab_to_front_and_sets_flag() {
    let mut col = TabsCollection::new();
    let _a = col.open(Some(PathBuf::from("/tmp/a.zip")));
    let target = col.open(Some(PathBuf::from("/tmp/target.zip")));
    let _c = col.open(Some(PathBuf::from("/tmp/c.zip")));
    col.set_pinned(target, true);
    assert_eq!(col.tabs()[0].id, target, "pinned tab moved to front");
    assert!(col.tabs()[0].pinned.load(Ordering::SeqCst));
    assert_eq!(col.pinned_count(), 1);
}

#[test]
fn set_pinned_preserves_pin_order_when_pinning_multiple() {
    let mut col = TabsCollection::new();
    let a = col.open(Some(PathBuf::from("/tmp/a.zip")));
    let b = col.open(Some(PathBuf::from("/tmp/b.zip")));
    col.set_pinned(a, true);
    col.set_pinned(b, true);
    // a pinned first → should be at index 0; b second → index 1.
    assert_eq!(col.tabs()[0].id, a);
    assert_eq!(col.tabs()[1].id, b);
    assert_eq!(col.pinned_count(), 2);
}

#[test]
fn set_pinned_unpin_moves_to_start_of_unpinned_section() {
    let mut col = TabsCollection::new();
    let pinned = col.open(Some(PathBuf::from("/tmp/p.zip")));
    let _unpinned = col.open(Some(PathBuf::from("/tmp/u.zip")));
    col.set_pinned(pinned, true);
    assert_eq!(col.pinned_count(), 1);
    col.set_pinned(pinned, false);
    assert_eq!(col.pinned_count(), 0);
    assert!(!col.get(pinned).unwrap().pinned.load(Ordering::SeqCst));
}

#[test]
fn set_pinned_unchanged_state_is_noop() {
    let mut col = TabsCollection::new();
    let id = col.open(Some(PathBuf::from("/tmp/x.zip")));
    let before: Vec<TabId> = col.tabs().iter().map(|t| t.id).collect();
    col.set_pinned(id, false); // already unpinned
    let after: Vec<TabId> = col.tabs().iter().map(|t| t.id).collect();
    assert_eq!(before, after);
}

#[test]
fn close_others_skips_pinned_tabs() {
    let mut col = TabsCollection::new();
    let pinned = col.open(Some(PathBuf::from("/tmp/pin.zip")));
    let keep = col.open(Some(PathBuf::from("/tmp/keep.zip")));
    let _other = col.open(Some(PathBuf::from("/tmp/other.zip")));
    col.set_pinned(pinned, true);
    let skipped = col.close_others(keep);
    assert_eq!(skipped, 1, "pinned tab counted as skipped");
    assert!(col.get(pinned).is_some());
    assert!(col.get(keep).is_some());
}

#[test]
fn reorder_blocks_cross_pinned_boundary() {
    let mut col = TabsCollection::new();
    let pinned = col.open(Some(PathBuf::from("/tmp/p.zip")));
    let _unpinned1 = col.open(Some(PathBuf::from("/tmp/u1.zip")));
    let _unpinned2 = col.open(Some(PathBuf::from("/tmp/u2.zip")));
    col.set_pinned(pinned, true);
    // After pinning, order is: [pinned, first-default, u1, u2]
    // Attempt to drag the last unpinned (idx 3) into the pinned area (idx 0).
    let before: Vec<TabId> = col.tabs().iter().map(|t| t.id).collect();
    col.reorder(3, 0);
    let after: Vec<TabId> = col.tabs().iter().map(|t| t.id).collect();
    assert_eq!(before, after, "cross-section reorder blocked");
}

#[test]
fn reorder_within_unpinned_section_works() {
    let mut col = TabsCollection::new();
    let pinned = col.open(Some(PathBuf::from("/tmp/p.zip")));
    let _u1 = col.open(Some(PathBuf::from("/tmp/u1.zip")));
    let _u2 = col.open(Some(PathBuf::from("/tmp/u2.zip")));
    col.set_pinned(pinned, true);
    // Move u1 (idx 2) past u2 (idx 3) → both unpinned, allowed.
    let u1_id = col.tabs()[2].id;
    col.reorder(2, 3);
    assert_eq!(col.tabs()[3].id, u1_id);
}
