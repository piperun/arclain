//! Render-time tests for the tab bar.
//!
//! The tab bar paints its chips and the close/plus icons directly via
//! the `egui::Painter` (no inner widgets) so it can control vertical
//! centering and lay out the close button inside the same pill as the
//! title. That means egui_kittest can't query for those glyphs by
//! label — they're not widgets, they're painted geometry. The tests
//! here therefore verify:
//!
//! 1. The renderer runs without panicking against several collection
//!    shapes (single tab, multi-tab, with in-flight ops set).
//! 2. The returned `Option<TabBarAction>` is `None` on first paint with
//!    no input (passive render).
//!
//! Click-routing logic (which TabBarAction comes out for which input)
//! is exercised through manual smoke testing and by the unit tests in
//! `tabs_collection_tests.rs` which cover the collection mutations the
//! actions feed into.

use arclain_theme::AppTheme;
use arclain_ui::core::tabs::TabsCollection;
use arclain_ui::shared::components::tab_bar::{render_tab_bar, TabBarAction};
use egui_kittest::Harness;
use std::sync::atomic::Ordering;

struct TabBarState {
    col: TabsCollection,
    last_action: Option<TabBarAction>,
    theme: AppTheme,
}

impl TabBarState {
    fn new() -> Self {
        Self {
            col: TabsCollection::new(),
            last_action: None,
            theme: AppTheme::default(),
        }
    }
}

#[test]
fn tab_bar_renders_with_one_active_tab() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut TabBarState| {
            let (action, _scroll) = render_tab_bar(ui, &state.col, &state.theme.colors);
            state.last_action = action;
        },
        TabBarState::new(),
    );
    harness.run();
    assert_eq!(harness.state().col.tabs().len(), 1);
    assert!(harness.state().last_action.is_none(),
            "passive render with no input should not produce an action");
}

#[test]
fn tab_bar_renders_with_multiple_tabs_without_panic() {
    let mut state = TabBarState::new();
    state.col.open(Some(std::path::PathBuf::from("/tmp/a.zip")));
    state.col.open(Some(std::path::PathBuf::from("/tmp/b.zip")));
    state.col.open(None);
    assert_eq!(state.col.tabs().len(), 4);

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut TabBarState| {
            let _ = render_tab_bar(ui, &state.col, &state.theme.colors);
        },
        state,
    );
    harness.run();
    assert_eq!(harness.state().col.tabs().len(), 4);
}

#[test]
fn tab_bar_renders_with_in_flight_ops_indicator_without_panic() {
    let mut state = TabBarState::new();
    state.col.open(Some(std::path::PathBuf::from("/tmp/busy.zip")));
    state.col.tabs()[0].in_flight_ops.store(3, Ordering::SeqCst);

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut TabBarState| {
            let _ = render_tab_bar(ui, &state.col, &state.theme.colors);
        },
        state,
    );
    harness.run();
    // Sanity: the in-flight counter is preserved.
    assert_eq!(
        harness.state().col.tabs()[0].in_flight_ops.load(Ordering::SeqCst),
        3
    );
}
