//! The toolbar's plugin buttons, end to end on the facade session path.
//!
//! `plugin_session_facade_test.rs` covers the session registry itself and
//! `plugin_document_render_test.rs` covers the document renderer; this
//! file covers the surface those two meet on for `PluginButton` -- the
//! real toolbar drawing a real plugin's real button, a press round-
//! tripping through `start_plugin_action`, and the layout editor offering
//! exactly the buttons the toolbar can draw.
//!
//! Uses the `ui-demo` fixture, the one workspace plugin that implements
//! the `PluginButton` extension point (one button, `plugin_toolbar_btn` /
//! "Plugin Action").

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use arclain_app::event::OperationKind;
use arclain_app::layout::{UiActionTypeDto, UiDisplayModeDto, UiItemDto, UiRegionDto};
use arclain_app::plugins::PluginUiDocument;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::operation_bridge;
use arclain_ui::features::plugins::application::{
    document_buttons, PluginSlot, PluginUiJobs, SlotView,
};
use arclain_ui::features::plugins::presentation::toolbar_item;
use arclain_ui::features::settings::presentation::pages::{
    handle_toolbar_layout_action, LayoutEditorAction, ToolbarLayoutState,
};
use arclain_ui::shared::components::toolbar::{self, ToolbarConfig};
use arclain_ui::shared::SharedState;
use eframe::egui;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use tempfile::TempDir;

const PLUGIN: &str = "ui-demo";
const BUTTON_ID: &str = "plugin_toolbar_btn";
const BUTTON_LABEL: &str = "Plugin Action";

// ============================================================================
// Fixture scaffolding.
// ============================================================================

/// Copies a workspace plugin fixture into the folder layout the plugin
/// loader expects -- mirrors `plugin_session_facade_test.rs`'s helper of
/// the same name (each test binary is its own crate).
/// A `SharedState` with a real application behind it *and* a real plugin
/// loaded, composed the way `core::state::init` composes the running app:
/// the frontend's legacy service handles come from the application's own
/// composition, so `plugin_ui_jobs` and the facade see one plugin manager
/// rather than two.
fn shared_state_with_plugin() -> (TempDir, SharedState) {
    let temp = tempfile::tempdir().expect("create tempdir for the test facade");
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    common::install_plugin_fixture(&paths.plugins_dir, PLUGIN);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("bootstrap the test facade");
    let mut shared = common::create_test_shared_state();
    shared.plugin_ui_jobs = PluginUiJobs::new(
        Some(app.clone()),
        shared.services.tokio_runtime.handle().clone(),
    );
    shared
        .app_state
        .lock()
        .reload_ui_config(&app, &shared.services.tokio_runtime);
    shared.facade = Some(app);
    (temp, shared)
}

/// Stores a toolbar item pointing at one of the plugin's buttons -- the
/// shape the layout editor persists (`"{plugin_id}:{button_id}"`).
fn seed_plugin_toolbar_item(shared: &SharedState, action_data: &str) {
    let app = shared.facade.as_ref().expect("the fixture has a facade");
    let runtime = &shared.services.tokio_runtime;
    let mut items = runtime
        .block_on(app.list_ui_items(UiRegionDto::Toolbar))
        .expect("list the stored toolbar items");
    let sort_order = items.iter().map(|item| item.sort_order).max().unwrap_or(0) + 10;
    items.push(UiItemDto {
        id: format!("plugin_{}", action_data.replace(':', "_")),
        region: UiRegionDto::Toolbar,
        group_id: Some("plugins".to_string()),
        label: format!("UI Demo Plugin - {BUTTON_LABEL}"),
        icon: Some("PUZZLE_PIECE".to_string()),
        action_type: UiActionTypeDto::Plugin,
        action_data: Some(action_data.to_string()),
        visible: true,
        sort_order,
        display_mode: UiDisplayModeDto::IconAndText,
    });
    runtime
        .block_on(app.save_ui_items(UiRegionDto::Toolbar, items))
        .expect("save the toolbar items");
    shared
        .app_state
        .lock()
        .reload_ui_config(app, &shared.services.tokio_runtime);
}

/// Pumps the legacy job queue until the plugin snapshot is cached, the
/// way a real frame does through `process_plugin_ui_results`.
///
/// The toolbar still reads that snapshot for one thing -- whether a
/// plugin is enabled -- so a frame that has never drained it draws no
/// plugin buttons at all, exactly as the pre-cutover path did.
fn warm_plugin_snapshot(shared: &SharedState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let _ = shared.plugin_ui_jobs.drain();
        if let Some(Ok(plugins)) = shared.plugin_ui_jobs.plugin_snapshot() {
            if plugins.iter().any(|plugin| plugin.id == PLUGIN) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the plugin snapshot never loaded"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Polls the plugin's `PluginButton` slot until its session is open, the
/// way a render loop does -- the first look spawns the open, later ones
/// observe it.
fn warm_plugin_button_slot(shared: &SharedState) -> Arc<PluginUiDocument> {
    warm_plugin_snapshot(shared);
    let facade = shared.facade.as_ref().expect("the fixture has a facade");
    let slot = PluginSlot::PluginButton {
        plugin_id: PLUGIN.to_string(),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match shared
            .plugin_sessions
            .view(facade, shared.services.tokio_runtime.handle(), &slot)
        {
            SlotView::Ready(document) => return document,
            SlotView::Failed(error) => panic!("the plugin-button slot failed to open: {error}"),
            SlotView::Opening => {
                assert!(
                    Instant::now() < deadline,
                    "the plugin session never finished opening"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// The production toolbar frame: the real `toolbar::render` over the
/// stored items, with the same one-line plugin bridge
/// `core::arclain_app::toolbar_handler` installs.
fn toolbar_harness(shared: &SharedState) -> Harness<'static> {
    let shared = shared.clone();
    Harness::new(move |ctx| {
        egui::TopBottomPanel::top("toolbar_panel").show(ctx, |ui| {
            let tab = shared.signals().tabs.get().active().clone();
            let mut view_state = tab.browser_view_state.get();
            let config = ToolbarConfig::new(shared.signals().toolbar_items.get());
            let shared_ref = &shared;
            let mut plugin_renderer =
                move |ui: &mut egui::Ui, plugin_id: &str, button_id: Option<&str>| {
                    toolbar_item::render_toolbar_item(ui, shared_ref, plugin_id, button_id);
                };
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
            tab.browser_view_state.set_if_changed(view_state);
        });
    })
}

/// Routes terminal plugin-action events into the session registry the way
/// `core::operation_bridge`'s subscriber does in a running app.
///
/// Without it, the only delivery path is the re-read
/// `document_dispatch::dispatch_action` performs immediately after
/// registering, and whether that re-read wins the race against the
/// action's own worker is genuinely nondeterministic -- so a test
/// asserting the result applies would flake rather than fail. Double
/// delivery is harmless: the second arrival finds the operation already
/// drained.
fn spawn_plugin_action_bridge(shared: &SharedState) {
    let facade = shared.facade.clone().expect("the fixture has a facade");
    let mut events = facade.subscribe_operations();
    let shared = shared.clone();
    shared
        .services
        .tokio_runtime
        .handle()
        .clone()
        .spawn(async move {
            while let Ok(event) = events.recv().await {
                if event.kind == OperationKind::PluginAction {
                    operation_bridge::handle_plugin_action_event(&shared, event);
                }
            }
        });
}

fn wait_for_revision_beyond(shared: &SharedState, previous: u64) -> u64 {
    let facade = shared.facade.as_ref().expect("the fixture has a facade");
    let slot = PluginSlot::PluginButton {
        plugin_id: PLUGIN.to_string(),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let SlotView::Ready(document) =
            shared
                .plugin_sessions
                .view(facade, shared.services.tokio_runtime.handle(), &slot)
        {
            if document.revision > previous {
                return document.revision;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the slot's document never advanced past revision {previous}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ============================================================================
// The toolbar draws what the plugin's document says.
// ============================================================================

#[test]
fn the_toolbar_draws_a_plugins_button_from_its_facade_document() {
    let (_temp, shared) = shared_state_with_plugin();
    seed_plugin_toolbar_item(&shared, &format!("{PLUGIN}:{BUTTON_ID}"));
    let document = warm_plugin_button_slot(&shared);
    assert_eq!(
        document_buttons(&document.root)
            .iter()
            .map(|button| (button.id, button.label))
            .collect::<Vec<_>>(),
        vec![(BUTTON_ID, BUTTON_LABEL)],
        "precondition: the fixture's PluginButton document offers one button"
    );

    let mut harness = toolbar_harness(&shared);
    harness.run();

    assert!(
        harness.query_by_label(BUTTON_LABEL).is_some(),
        "the toolbar must draw the plugin's button, labelled as the document labels it"
    );
}

/// A stored item naming a button the plugin does not offer draws nothing,
/// rather than an unlabelled control the user cannot act on.
#[test]
fn an_item_naming_an_unknown_button_draws_nothing() {
    let (_temp, shared) = shared_state_with_plugin();
    seed_plugin_toolbar_item(&shared, &format!("{PLUGIN}:no_such_button"));
    warm_plugin_button_slot(&shared);

    let mut harness = toolbar_harness(&shared);
    harness.run();

    assert!(harness.query_by_label(BUTTON_LABEL).is_none());
}

/// A stored item outlives the plugin being enabled, so an item for a
/// plugin the user has since disabled must draw nothing.
///
/// The application also refuses to open a disabled plugin session. The
/// toolbar's own enabled check is still worth pinning: it should avoid
/// declaring a doomed slot at all, rather than opening one merely to
/// cache the facade's terminal refusal.
#[test]
fn a_disabled_plugins_button_is_not_drawn() {
    let (_temp, shared) = shared_state_with_plugin();
    seed_plugin_toolbar_item(&shared, &format!("{PLUGIN}:{BUTTON_ID}"));
    warm_plugin_button_slot(&shared);

    let app = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .services
        .tokio_runtime
        .block_on(app.set_plugin_enabled(PLUGIN.to_string(), false))
        .expect("disable the plugin");
    // What the enable toggle does in the running app.
    shared.plugin_ui_jobs.invalidate_plugin_snapshots();
    shared
        .plugin_sessions
        .close_plugin(app, shared.services.tokio_runtime.handle(), PLUGIN);
    warm_plugin_snapshot(&shared);

    let mut harness = toolbar_harness(&shared);
    harness.run();

    assert!(
        harness.query_by_label(BUTTON_LABEL).is_none(),
        "a disabled plugin's stored toolbar item must draw nothing"
    );
    assert!(
        shared.plugin_sessions.is_empty(),
        "the toolbar must reject the disabled item before declaring a facade slot"
    );
}

/// A stored item that names the plugin rather than one of its buttons --
/// the shape the pre-cutover "legacy multi-button" branch handled -- draws
/// the plugin's whole toolbar document.
#[test]
fn an_item_naming_the_whole_plugin_draws_its_whole_document() {
    let (_temp, shared) = shared_state_with_plugin();
    seed_plugin_toolbar_item(&shared, PLUGIN);
    warm_plugin_button_slot(&shared);

    let mut harness = toolbar_harness(&shared);
    harness.run();

    assert!(harness.query_by_label(BUTTON_LABEL).is_some());
}

// ============================================================================
// A press reaches the plugin, and the result lands on the slot.
// ============================================================================

#[test]
fn pressing_a_plugin_toolbar_button_dispatches_it_and_the_result_applies() {
    let (_temp, shared) = shared_state_with_plugin();
    seed_plugin_toolbar_item(&shared, &format!("{PLUGIN}:{BUTTON_ID}"));
    let opened = warm_plugin_button_slot(&shared);
    spawn_plugin_action_bridge(&shared);

    let mut harness = toolbar_harness(&shared);
    harness.run();
    harness.get_by_label(BUTTON_LABEL).click();
    harness.run();

    let applied = wait_for_revision_beyond(&shared, opened.revision);
    assert!(
        applied > opened.revision,
        "the press must round-trip through the plugin and advance the slot's document"
    );
}

// ============================================================================
// The layout editor offers exactly what the toolbar can draw.
// ============================================================================

#[test]
fn the_layout_editor_discovers_the_same_buttons_from_the_normalized_tree() {
    let (_temp, shared) = shared_state_with_plugin();
    let document = warm_plugin_button_slot(&shared);
    let offered_by_document: Vec<(String, String)> = document_buttons(&document.root)
        .iter()
        .map(|button| {
            (
                format!("plugin_{PLUGIN}_{}", button.id),
                format!("{PLUGIN}:{}", button.id),
            )
        })
        .collect();
    assert!(!offered_by_document.is_empty());

    let mut editor = ToolbarLayoutState::default();
    // The sync runs every frame the editor is open and needs the plugin
    // snapshot as well as the document, so drive it the way the page does
    // rather than assuming one pass is enough. Draining the job queue is
    // what a frame does through `process_plugin_ui_results`; only its
    // cache-filling side effect matters here, since the editor reads the
    // snapshot cache rather than the results themselves.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let _ = shared.plugin_ui_jobs.drain();
        handle_toolbar_layout_action(&mut editor, LayoutEditorAction::SyncItems, &shared);
        if editor
            .items
            .iter()
            .any(|item| item.action_type == UiActionTypeDto::Plugin)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the editor never discovered the plugin's toolbar buttons"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let discovered: Vec<(String, String)> = editor
        .items
        .iter()
        .filter(|item| item.action_type == UiActionTypeDto::Plugin)
        .map(|item| {
            (
                item.id.clone(),
                item.action_data.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        discovered, offered_by_document,
        "the editor must offer exactly the buttons the toolbar's own document carries"
    );

    // And every offered item resolves back to a real node, which is what
    // makes "offered" and "drawable" the same set rather than two lists
    // that happen to agree today.
    for (_, action_data) in &discovered {
        let (_, button_id) = action_data.split_once(':').expect("item names a button");
        assert!(
            document.root.find(button_id).is_some(),
            "{button_id} must resolve in the document the toolbar draws from"
        );
    }
}

// ============================================================================
// `shared/` stays plugin-agnostic.
// ============================================================================

/// The seam itself: the shared toolbar parses its own stored item and
/// hands the host a plugin id plus the button that item names. Both
/// stored shapes are covered, because the pre-cutover module handled them
/// in two branches that read the plugin's elements two different ways.
#[test]
fn the_shared_toolbar_hands_the_host_the_parsed_item_and_nothing_else() {
    fn plugin_item(id: &str, action_data: &str, sort_order: i32) -> UiItemDto {
        UiItemDto {
            id: id.to_string(),
            region: UiRegionDto::Toolbar,
            group_id: Some("plugins".to_string()),
            label: "Plugin".to_string(),
            icon: None,
            action_type: UiActionTypeDto::Plugin,
            action_data: Some(action_data.to_string()),
            visible: true,
            sort_order,
            display_mode: UiDisplayModeDto::IconAndText,
        }
    }

    let shared = common::create_test_shared_state();
    let config = ToolbarConfig::new(vec![
        plugin_item("named", "demo-plugin:some_button", 10),
        plugin_item("whole", "other-plugin", 20),
    ]);
    let mut state = toolbar::ToolbarState::default();
    let mut seen: Vec<(String, Option<String>)> = Vec::new();

    let mut harness = Harness::new_ui(|ui| {
        // The harness runs several frames; each one draws the whole
        // toolbar, so this records one frame's worth rather than a
        // frame-count-dependent pile.
        seen.clear();
        let mut plugin_renderer = |_: &mut egui::Ui, plugin_id: &str, button_id: Option<&str>| {
            seen.push((plugin_id.to_string(), button_id.map(str::to_string)));
        };
        let _ = toolbar::render(
            ui,
            &shared.theme,
            &mut state,
            false,
            false,
            false,
            true,
            false,
            false,
            Some(&config),
            None,
            &mut plugin_renderer,
        );
    });
    harness.run();
    drop(harness);

    assert_eq!(
        seen,
        vec![
            ("demo-plugin".to_string(), Some("some_button".to_string())),
            ("other-plugin".to_string(), None),
        ]
    );
}

/// The toolbar is a shared component: it draws stored layout items and
/// takes everything else through injected callbacks. Naming a plugin type
/// there is what this cutover removed, and re-introducing one would put
/// the plugin stack back inside a module every other host also renders.
///
/// Checked against the source rather than the type system because the
/// violation is an *import*, which nothing else can assert. Comment lines
/// are excluded: this module's own doc comments explain the seam by name,
/// which is exactly what a future reader needs.
#[test]
fn the_shared_toolbar_module_names_no_plugin_type() {
    let modules = [
        (
            "toolbar/mod.rs",
            include_str!("../src/shared/components/toolbar/mod.rs"),
        ),
        (
            "toolbar/types.rs",
            include_str!("../src/shared/components/toolbar/types.rs"),
        ),
        (
            "toolbar/buttons.rs",
            include_str!("../src/shared/components/toolbar/buttons.rs"),
        ),
    ];
    for (name, source) in modules {
        for (index, line) in source.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for forbidden in ["arclain_plugins", "features::plugins"] {
                assert!(
                    !code.contains(forbidden),
                    "{name}:{} names `{forbidden}`; the shared toolbar must reach \
                     the plugin stack only through its injected renderer",
                    index + 1
                );
            }
        }
    }
}

/// The production wiring, pinned the way `archive_browser_test.rs` pins
/// the toolbar's change-gated view publication: the harness above builds
/// the plugin bridge itself, so this is what says the running app builds
/// the same one.
#[test]
fn the_production_toolbar_installs_the_facade_backed_plugin_renderer() {
    let source = include_str!("../src/core/arclain_app/toolbar_handler.rs");
    assert!(
        source.contains("toolbar_item::render_toolbar_item(ui, shared_ref, plugin_id, button_id)")
    );
}
