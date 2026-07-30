//! What the chrome layout looks like once it comes through the
//! application: the toolbar draws the stored arrangement, and the layout
//! editor round-trips an edit of it.
//!
//! `dispatcher_test.rs` covers the dispatchers' own branches (dirty
//! guards, missing application, per-item toggles); this file covers the
//! two ends the user actually sees — the rendered toolbar and a save that
//! survives a reload.
//!
//! Kept in its own file rather than appended to `archive_browser_test.rs`
//! (which already renders a toolbar for an unrelated reason): concurrent
//! worktrees also edit that file.

mod common;

use arclain_app::layout::{UiItemDto, UiRegionDto};
use arclain_ui::features::settings::presentation::pages::{
    handle_info_panel_layout_action, handle_toolbar_layout_action, InfoPanelLayoutState,
    LayoutEditorAction, ToolbarLayoutState,
};
use arclain_ui::shared::components::toolbar::{self, ToolbarConfig};
use arclain_ui::shared::SharedState;
use common::create_test_shared_state_with_facade;
use eframe::egui;
use egui_kittest::Harness;

// ============================================================================
// Harness helpers.
// ============================================================================

fn stored_items(shared: &SharedState, region: UiRegionDto) -> Vec<UiItemDto> {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .services
        .tokio_runtime
        .block_on(app.list_ui_items(region))
        .expect("list the stored items")
}

fn save_items(shared: &SharedState, region: UiRegionDto, items: Vec<UiItemDto>) {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .services
        .tokio_runtime
        .block_on(app.save_ui_items(region, items))
        .expect("save the items");
}

/// Refreshes the canonical item signals the way the settings header does
/// after a save lands.
fn reload_signals(shared: &SharedState) {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .app_state
        .lock()
        .reload_ui_config(app, &shared.services.tokio_runtime);
}

/// What the toolbar would draw, as (group, item ids in draw order) —
/// exactly the structure `toolbar::render` iterates.
fn drawn_groups(shared: &SharedState) -> Vec<(Option<String>, Vec<String>)> {
    ToolbarConfig::new(shared.signals().toolbar_items.get())
        .items_by_group()
        .into_iter()
        .map(|(group, items)| {
            (
                group,
                items.into_iter().map(|item| item.id.clone()).collect(),
            )
        })
        .collect()
}

/// Renders the real toolbar from the current signal for a couple of
/// frames. Its job is to prove the arrangement the assertions above talk
/// about is the one a live frame actually draws, rather than a model only
/// the test looks at.
fn render_toolbar(shared: &SharedState) {
    let shared = shared.clone();
    let mut harness = Harness::new(move |ctx| {
        egui::TopBottomPanel::top("toolbar_panel").show(ctx, |ui| {
            let tab = shared.signals().tabs.get().active().clone();
            let mut view_state = tab.browser_view_state.get();
            let config = ToolbarConfig::new(shared.signals().toolbar_items.get());
            let mut plugin_renderer = |_: &mut egui::Ui, _: &str, _: Option<&str>| {};
            let _ = toolbar::render(
                ui,
                &shared.theme,
                &mut view_state.toolbar_state,
                false,
                false,
                false,
                true,
                false,
                false,
                Some(&config),
                Some(&shared),
                &mut plugin_renderer,
            );
        });
    });
    harness.run_steps(2);
}

// ============================================================================
// The toolbar draws the stored arrangement.
// ============================================================================

#[test]
fn the_toolbar_draws_the_seeded_arrangement() {
    let (_temp, shared) = create_test_shared_state_with_facade();

    assert_eq!(
        drawn_groups(&shared),
        vec![
            (
                Some("navigation".to_string()),
                vec![
                    "toolbar.back".to_string(),
                    "toolbar.forward".to_string(),
                    "toolbar.up".to_string(),
                ]
            ),
            (
                Some("file_actions".to_string()),
                vec![
                    "toolbar.open".to_string(),
                    "toolbar.extract".to_string(),
                    "toolbar.extract_all".to_string(),
                    "toolbar.add".to_string(),
                    "toolbar.delete".to_string(),
                    "toolbar.convert".to_string(),
                    "toolbar.batch_convert".to_string(),
                    "toolbar.organize".to_string(),
                ]
            ),
            (
                Some("view".to_string()),
                vec![
                    "toolbar.list_view".to_string(),
                    "toolbar.grid_view".to_string(),
                    "toolbar.column_lock".to_string(),
                ]
            ),
            (
                Some("panels".to_string()),
                vec![
                    "toolbar.tree_panel".to_string(),
                    "toolbar.properties_panel".to_string(),
                ]
            ),
        ],
        "the toolbar draws the shipped default arrangement, in its stored order"
    );

    render_toolbar(&shared);
}

#[test]
fn an_item_hidden_through_the_application_stops_being_drawn() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let before = drawn_groups(&shared);

    let mut items = stored_items(&shared, UiRegionDto::Toolbar);
    let hidden_id = items
        .iter_mut()
        .find(|item| item.id == "toolbar.batch_convert")
        .map(|item| {
            item.visible = false;
            item.id.clone()
        })
        .expect("the seeded batch-convert button");
    save_items(&shared, UiRegionDto::Toolbar, items);
    reload_signals(&shared);

    let after = drawn_groups(&shared);
    assert_ne!(before, after, "hiding an item must change what is drawn");
    assert!(
        !after.iter().any(|(_, ids)| ids.contains(&hidden_id)),
        "a hidden item is not drawn"
    );
    // Only that one item moved: every other group is untouched, and the
    // group it belonged to lost exactly it.
    let expected: Vec<(Option<String>, Vec<String>)> = before
        .into_iter()
        .map(|(group, ids)| {
            (
                group,
                ids.into_iter().filter(|id| id != &hidden_id).collect(),
            )
        })
        .collect();
    assert_eq!(after, expected);

    render_toolbar(&shared);
}

#[test]
fn a_reordered_arrangement_is_drawn_in_the_new_order() {
    let (_temp, shared) = create_test_shared_state_with_facade();

    let mut items = stored_items(&shared, UiRegionDto::Toolbar);
    // Exchange the first two navigation items' sort order, the way the
    // editor's move buttons do.
    let first = items[0].sort_order;
    items[0].sort_order = items[1].sort_order;
    items[1].sort_order = first;
    save_items(&shared, UiRegionDto::Toolbar, items);
    reload_signals(&shared);

    let navigation = drawn_groups(&shared)
        .into_iter()
        .find(|(group, _)| group.as_deref() == Some("navigation"))
        .expect("the navigation group")
        .1;
    assert_eq!(
        navigation,
        vec![
            "toolbar.forward".to_string(),
            "toolbar.back".to_string(),
            "toolbar.up".to_string(),
        ],
        "the drawn order follows the stored sort order"
    );

    render_toolbar(&shared);
}

// ============================================================================
// The layout editor round-trips.
// ============================================================================

/// Edit → save → reload → a fresh editor sees exactly the edit. The
/// pre-facade editor persisted through a service handle; this is the same
/// contract through the application.
#[test]
fn the_toolbar_editor_round_trips_an_edited_arrangement() {
    let (_temp, shared) = create_test_shared_state_with_facade();

    let mut editor = ToolbarLayoutState::default();
    handle_toolbar_layout_action(&mut editor, LayoutEditorAction::SyncItems, &shared);
    assert!(editor.loaded);
    assert!(
        !editor.items.is_empty(),
        "the editor loaded the seeded layout"
    );
    assert!(!editor.dirty);

    // A reorder and a hide, marked dirty the way the render path marks it.
    let first = editor.items[0].sort_order;
    editor.items[0].sort_order = editor.items[1].sort_order;
    editor.items[1].sort_order = first;
    editor.items[2].visible = false;
    editor.dirty = true;
    let mut expected = editor.items.clone();
    expected.sort_by_key(|item| item.sort_order);

    editor.save(&shared).expect("the save must succeed");
    assert!(!editor.dirty, "a successful save clears the dirty flag");
    reload_signals(&shared);

    let mut reopened = ToolbarLayoutState::default();
    handle_toolbar_layout_action(&mut reopened, LayoutEditorAction::SyncItems, &shared);
    assert_eq!(
        reopened.items, expected,
        "reopening the editor shows exactly what was saved"
    );
    assert!(
        !reopened.dirty,
        "and reopening a saved arrangement is not itself an edit"
    );
}

#[test]
fn the_info_panel_editor_round_trips_an_edited_arrangement() {
    let (_temp, shared) = create_test_shared_state_with_facade();

    let mut editor = InfoPanelLayoutState::default();
    handle_info_panel_layout_action(&mut editor, LayoutEditorAction::SyncItems, &shared);
    assert!(!editor.items.is_empty());

    editor.items[0].visible = false;
    editor.items[1].sort_order += 100;
    editor.dirty = true;
    let mut expected = editor.items.clone();
    expected.sort_by_key(|item| item.sort_order);

    editor.save(&shared).expect("the save must succeed");
    reload_signals(&shared);

    let mut reopened = InfoPanelLayoutState::default();
    handle_info_panel_layout_action(&mut reopened, LayoutEditorAction::SyncItems, &shared);
    assert_eq!(reopened.items, expected);
}

/// One editor saves one region. The pre-facade save wrote whatever list
/// it held with no region check at all, so this pins the guarantee that
/// replaced it.
#[test]
fn saving_one_region_leaves_the_others_alone() {
    let (_temp, shared) = create_test_shared_state_with_facade();
    let info_panel_before = stored_items(&shared, UiRegionDto::InfoPanel);
    let context_menu_before = stored_items(&shared, UiRegionDto::ContextMenu);

    let mut editor = ToolbarLayoutState::default();
    handle_toolbar_layout_action(&mut editor, LayoutEditorAction::SyncItems, &shared);
    editor.items[0].visible = false;
    editor.dirty = true;
    editor.save(&shared).expect("the save must succeed");

    assert_eq!(
        stored_items(&shared, UiRegionDto::InfoPanel),
        info_panel_before
    );
    assert_eq!(
        stored_items(&shared, UiRegionDto::ContextMenu),
        context_menu_before
    );
}

/// A save with no application behind it reports the failure and leaves
/// the editor dirty, so the user can retry instead of watching their
/// arrangement disappear.
#[test]
fn a_save_with_no_application_is_reported_and_keeps_the_edit() {
    let shared = common::create_test_shared_state();

    let mut editor = ToolbarLayoutState::default();
    editor.items = vec![UiItemDto {
        id: "toolbar.example".to_string(),
        region: UiRegionDto::Toolbar,
        group_id: None,
        label: "Example".to_string(),
        icon: None,
        visible: true,
        sort_order: 0,
        display_mode: Default::default(),
        action_type: Default::default(),
        action_data: None,
    }];
    editor.dirty = true;

    let error = editor.save(&shared).expect_err("there is no application");
    assert!(!error.is_empty());
    assert!(
        editor.dirty,
        "a failed save must not clear the dirty flag -- the edit is still unsaved"
    );
    assert_eq!(
        editor.items.len(),
        1,
        "and must not discard the edit either"
    );
}
