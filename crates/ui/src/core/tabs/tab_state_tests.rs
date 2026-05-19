// crates/ui/src/core/tabs/tab_state_tests.rs

use super::*;
use std::sync::atomic::Ordering;

#[test]
fn default_state_signals_are_initialized() {
    let tab = TabState::new(TabId(42));
    assert_eq!(tab.id, TabId(42));
    assert!(tab.archive_path.get().is_none());
    assert!(tab.entries.get().is_empty());
    assert!(tab.metadata.get().is_none());
    assert!(!tab.loading.get());
    assert!(tab.ui_ready.get()); // Starts true
    assert!(tab.opened_archive.get().is_none());
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 0);
    assert!(!tab.tab_cancel.load(Ordering::SeqCst));
}

#[test]
fn in_flight_counter_increments_decrements() {
    let tab = TabState::new(TabId(1));
    tab.in_flight_ops.fetch_add(1, Ordering::SeqCst);
    tab.in_flight_ops.fetch_add(1, Ordering::SeqCst);
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 2);
    tab.in_flight_ops.fetch_sub(1, Ordering::SeqCst);
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 1);
}

#[test]
fn display_title_derives_from_path() {
    let tab = TabState::new(TabId(1));
    tab.archive_path
        .set(Some(PathBuf::from("/some/dir/Example.zip")));
    assert_eq!(tab.display_title(), "Example");
}

#[test]
fn display_title_empty_when_no_path() {
    let tab = TabState::new(TabId(1));
    assert_eq!(tab.display_title(), "New tab");
}

#[test]
fn display_title_handles_no_extension() {
    let tab = TabState::new(TabId(1));
    tab.archive_path.set(Some(PathBuf::from("/no/extension/here")));
    assert_eq!(tab.display_title(), "here");
}

#[test]
fn arc_clone_shares_signal_state() {
    // Sanity: Signal is Arc-backed under the hood, so clones share state.
    // Background ops capture Arc<TabState>; mutations through the clone
    // must be visible through the original (and vice versa).
    let tab = Arc::new(TabState::new(TabId(1)));
    let tab2 = Arc::clone(&tab);
    tab2.archive_path.set(Some(PathBuf::from("/x.zip")));
    assert_eq!(tab.archive_path.get(), Some(PathBuf::from("/x.zip")));
}
