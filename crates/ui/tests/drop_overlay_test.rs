//! egui_kittest UI tests for the drop overlay.

// NOTE: kittest 0.3.0 API disambiguation:
//   - `query_all_by_label` — returns an iterator, empty if no matches (use for existence checks)
//   - `get_all_by_label`   — panics if no matches (use only when label is guaranteed present)
use arclain_ui::core::tabs::TabsCollection;
use arclain_ui::shared::components::drop_overlay::{render_drop_overlay, DropZone};
use eframe::egui;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use std::path::PathBuf;

#[test]
fn overlay_renders_two_zones_when_active_tab_has_archive() {
    let col = TabsCollection::new();
    col.active()
        .archive_path
        .set(Some(PathBuf::from("/tmp/x.zip")));
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut (TabsCollection, Option<DropZone>)| {
            let action = render_drop_overlay(ui, &state.0, Some(egui::pos2(100.0, 100.0)));
            state.1 = action;
        },
        (col, None::<DropZone>),
    );
    harness.run();
    // Verify both zone labels render as queryable widgets.
    assert!(
        harness
            .query_all_by_label("Open as new tab")
            .next()
            .is_some(),
        "Open as new tab label should be present"
    );
    assert!(
        harness
            .query_all_by_label("Replace current tab")
            .next()
            .is_some(),
        "Replace current tab label should be present"
    );
}

#[test]
fn overlay_renders_one_zone_when_active_tab_is_empty() {
    let col = TabsCollection::new(); // default: 1 empty tab, no archive_path
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut (TabsCollection, Option<DropZone>)| {
            let action = render_drop_overlay(ui, &state.0, Some(egui::pos2(100.0, 100.0)));
            state.1 = action;
        },
        (col, None::<DropZone>),
    );
    harness.run();
    assert!(harness
        .query_all_by_label("Open as new tab")
        .next()
        .is_some());
    // Replace zone hidden because active tab has no archive.
    assert!(
        harness
            .query_all_by_label("Replace current tab")
            .next()
            .is_none(),
        "Replace zone should NOT be rendered when active tab has no archive"
    );
}

#[test]
fn overlay_shows_ctrl_hint_in_replace_zone() {
    // Ctrl-held drops always route to Replace regardless of cursor
    // zone. The hint surfaces the keybinding so users don't need to
    // discover it from docs.
    let col = TabsCollection::new();
    col.active()
        .archive_path
        .set(Some(PathBuf::from("/tmp/x.zip")));
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut (TabsCollection, Option<DropZone>)| {
            let action = render_drop_overlay(ui, &state.0, Some(egui::pos2(100.0, 100.0)));
            state.1 = action;
        },
        (col, None::<DropZone>),
    );
    harness.run();
    assert!(
        harness.query_all_by_label("Hold Ctrl").next().is_some(),
        "Replace zone should display the 'Hold Ctrl' keybinding hint"
    );
}

#[test]
fn overlay_omits_ctrl_hint_when_no_replace_zone() {
    // No archive in the active tab → no Replace zone → no Ctrl hint.
    let col = TabsCollection::new();
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut (TabsCollection, Option<DropZone>)| {
            let action = render_drop_overlay(ui, &state.0, Some(egui::pos2(100.0, 100.0)));
            state.1 = action;
        },
        (col, None::<DropZone>),
    );
    harness.run();
    assert!(
        harness.query_all_by_label("Hold Ctrl").next().is_none(),
        "Ctrl hint should be absent when Replace zone isn't rendered"
    );
}

#[test]
fn overlay_returns_none_when_drop_pos_is_none() {
    let col = TabsCollection::new();
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut (TabsCollection, Option<DropZone>)| {
            let action = render_drop_overlay(ui, &state.0, None);
            state.1 = action;
        },
        (col, None::<DropZone>),
    );
    harness.run();
    assert!(
        harness.state().1.is_none(),
        "No drop position should yield no zone routing"
    );
}
