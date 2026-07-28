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
    assert!(tab.opened_archive.get().is_none());
    assert!(tab.pending_challenge.get().is_none());
    assert_eq!(tab.in_flight_ops.load(Ordering::SeqCst), 0);
    assert!(!tab.tab_cancel.load(Ordering::SeqCst));
    // Auto-binding flag must start false so the first bind sweep
    // actually subscribes listeners. See `bind_to_context_once`.
    assert!(!tab.signals_bound.load(Ordering::SeqCst));
}

#[test]
fn bind_to_context_is_idempotent() {
    // Regression test for the "drop-zip shows empty list until
    // I click somewhere" bug class. The design flaw was that
    // per-tab signals had no ctx-repaint listeners — background
    // writes landed silently. Fix: TabState::bind_to_context_once
    // subscribes every per-tab signal. Guarded by signals_bound
    // (AtomicBool) so re-firing the sweep on every tabs.set
    // doesn't stack duplicate listeners (which would multiply
    // repaint requests on every write).
    let tab = TabState::new(TabId(1));
    let ctx = egui::Context::default();

    assert!(!tab.signals_bound.load(Ordering::SeqCst));

    tab.bind_to_context_once(&ctx);
    assert!(
        tab.signals_bound.load(Ordering::SeqCst),
        "first bind must flip the flag"
    );

    // Second call MUST early-return — if it stacked another
    // round of bind_named on every signal, every signal write
    // would notify 2x, 3x, … on subsequent calls. Verify the
    // flag stays true (i.e. swap returned the previous value
    // and the bind body was skipped).
    tab.bind_to_context_once(&ctx);
    assert!(tab.signals_bound.load(Ordering::SeqCst));

    // And third, just to nail the contract.
    tab.bind_to_context_once(&ctx);
    assert!(tab.signals_bound.load(Ordering::SeqCst));
}

#[test]
fn bind_to_context_subscribes_repaint_on_signal_write() {
    // Behavioural half of the regression test. After binding,
    // writing a per-tab signal must trigger an egui repaint
    // request — without this, drop-zip and password-dialog show
    // would land silently.
    //
    // egui's `Context::has_requested_repaint` reflects whether
    // any subscriber called `request_repaint()` since the last
    // frame. We bind, write a signal, then check the flag.
    let tab = TabState::new(TabId(1));
    let ctx = egui::Context::default();
    tab.bind_to_context_once(&ctx);

    // Drive a frame so any pre-bind repaint requests are cleared
    // (Context::default starts with an implicit first-frame
    // repaint pending). The closure returns the test value we
    // care about — egui's `run` handles the frame lifecycle.
    let _ = ctx.run(Default::default(), |_| {});

    // Now perform the write that should trigger a repaint.
    tab.archive_path
        .set(Some(PathBuf::from("/test/archive.zip")));

    assert!(
        ctx.has_requested_repaint(),
        "writing tab.archive_path after bind_to_context_once must \
         request a UI repaint (drop-zip regression class)"
    );
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
    tab.archive_path
        .set(Some(PathBuf::from("/no/extension/here")));
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
