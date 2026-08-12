//! End-to-end coverage for the plugin `Page` extension point on the
//! facade session path.
//!
//! A page is the one surface whose first document cannot be drawn as soon
//! as `open_plugin_session` returns: its `__page_init` lifecycle event
//! must run first, and the action's returned revision is the first valid
//! document. These tests drive the real `render_page` host against the
//! deterministic WASM fixture, so they cover that ordering together with
//! page-stack/session lifetime and ordinary document dispatch.

mod common;

use std::time::{Duration, Instant};

use arclain_app::ids::PluginSessionId;
use arclain_app::plugins::{PluginExtensionPointDto, PluginUiDocument};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::app_lifecycle;
use arclain_ui::core::operation_bridge;
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{PluginSlot, SlotView};
use arclain_ui::features::plugins::presentation::views::rendering;
use arclain_ui::shared::SharedState;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use tempfile::TempDir;

const PLUGIN: &str = "facade-test-fixture";
const PAGE: &str = "fixture-page";
const CHILD_PAGE: &str = "fixture-child";
const PAGE_ACTION: &str = "Page Multi Action";
const OPEN_CHILD: &str = "Page Open Child";
const CLOSE_PAGE: &str = "Page Close";

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
    shared.facade = Some(app);
    operation_bridge::spawn(&shared);
    (temp, shared)
}

fn active_tab(shared: &SharedState) -> TabId {
    shared.signals().tabs.get().active_id()
}

fn page_slot(page_id: &str, tab: TabId) -> PluginSlot {
    PluginSlot::Page {
        plugin_id: PLUGIN.to_string(),
        page_id: page_id.to_string(),
        tab,
    }
}

fn open_page(shared: &SharedState, page_id: &str, tab: TabId) {
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.open_page(PLUGIN, page_id, tab);
    signal.set(state);
}

fn page_harness(shared: &SharedState) -> Harness<'static> {
    let shared = shared.clone();
    Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build(move |ctx| {
            rendering::render_page(ctx, &shared);
        })
}

fn step_until(
    harness: &mut Harness<'static>,
    what: &str,
    condition: impl Fn(&Harness<'static>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        harness.step();
        if condition(harness) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn step_until_page_ready(
    harness: &mut Harness<'static>,
    shared: &SharedState,
    page_id: &str,
    expected_label: &str,
) -> PluginSessionId {
    let slot = page_slot(page_id, active_tab(shared));
    step_until(
        harness,
        "the page never drew its post-init facade document",
        |harness| harness.query_by_label(expected_label).is_some(),
    );
    harness.run();
    shared
        .plugin_sessions
        .session_id(&slot)
        .expect("the rendered page must own a facade session")
}

fn document(shared: &SharedState, session_id: PluginSessionId) -> PluginUiDocument {
    shared
        .services
        .tokio_runtime
        .block_on(
            shared
                .facade
                .as_ref()
                .expect("the fixture has a facade")
                .plugin_ui_document(session_id),
        )
        .expect("the page session must have a document")
}

#[test]
fn the_page_draws_only_the_post_init_facade_document() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    open_page(&shared, PAGE, tab);

    let mut harness = page_harness(&shared);
    let session_id = step_until_page_ready(&mut harness, &shared, PAGE, "page-layout-call-2");

    assert!(
        harness.query_by_label("page-layout-call-1").is_none(),
        "the document fetched before __page_init must never reach the screen"
    );
    assert!(
        harness
            .query_by_label("Fixture Page (fixture-page)")
            .is_some(),
        "the page-init host intent must set the page title on its origin tab"
    );
    let document = document(&shared, session_id);
    assert_eq!(document.revision, 2);
    assert_eq!(
        document.extension_point,
        PluginExtensionPointDto::Page(PAGE.to_string())
    );
    assert!(!shared
        .signals()
        .plugin_dialog_state
        .get()
        .page_init_pending());
}

#[test]
fn re_rendering_an_open_page_never_refetches_its_layout() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    open_page(&shared, PAGE, tab);

    let mut harness = page_harness(&shared);
    let session_id = step_until_page_ready(&mut harness, &shared, PAGE, "page-layout-call-2");
    for _ in 0..10 {
        harness.step();
    }

    assert!(harness.query_by_label("page-layout-call-2").is_some());
    assert!(harness.query_by_label("page-layout-call-3").is_none());
    assert_eq!(
        shared.plugin_sessions.session_id(&page_slot(PAGE, tab)),
        Some(session_id)
    );
}

#[test]
fn a_page_press_dispatches_through_the_facade_and_applies_its_result() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    open_page(&shared, PAGE, tab);

    let mut harness = page_harness(&shared);
    let session_id = step_until_page_ready(&mut harness, &shared, PAGE, "page-layout-call-2");
    let opened_revision = document(&shared, session_id).revision;

    harness.get_by_label(PAGE_ACTION).click();
    harness.run();
    step_until(
        &mut harness,
        "the page press never round-tripped through the plugin",
        |_| {
            shared
                .signals()
                .tabs
                .get()
                .get(tab)
                .and_then(|tab| tab.page_display_name.get())
                .as_deref()
                == Some("third")
        },
    );

    assert!(document(&shared, session_id).revision > opened_revision);
    assert!(shared.plugin_sessions.tracked_ids().is_empty());
}

#[test]
fn a_close_page_button_releases_the_visible_pages_session() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    open_page(&shared, PAGE, tab);

    let mut harness = page_harness(&shared);
    step_until_page_ready(&mut harness, &shared, PAGE, "page-layout-call-2");
    harness.get_by_label(CLOSE_PAGE).click();
    harness.run();
    harness.step();

    assert!(!shared.signals().plugin_dialog_state.get().has_open_page());
    assert_eq!(
        shared.plugin_sessions.session_id(&page_slot(PAGE, tab)),
        None
    );
    assert!(shared.plugin_sessions.is_empty());
}

#[test]
fn pushing_and_popping_a_page_keeps_only_the_visible_pages_session() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    open_page(&shared, PAGE, tab);

    let mut harness = page_harness(&shared);
    step_until_page_ready(&mut harness, &shared, PAGE, "page-layout-call-2");

    harness.get_by_label(OPEN_CHILD).click();
    harness.run();
    step_until(
        &mut harness,
        "the child page never became current",
        |harness| {
            harness.query_by_label("fixture-child").is_some()
                && harness.query_by_label("page-layout-call-4").is_some()
        },
    );
    assert_eq!(
        shared.plugin_sessions.session_id(&page_slot(PAGE, tab)),
        None,
        "a covered page has no visible host and must release its session"
    );
    assert!(shared
        .plugin_sessions
        .session_id(&page_slot(CHILD_PAGE, tab))
        .is_some());

    harness.get_by_label(CLOSE_PAGE).click();
    harness.run();
    step_until(
        &mut harness,
        "the parent page never reopened after its child closed",
        |harness| {
            harness.query_all_by_label("fixture-page").next().is_some()
                && harness.query_by_label("page-layout-call-5").is_some()
        },
    );

    assert!(
        harness.query_by_label("page-layout-call-6").is_none(),
        "returning to an already initialized parent must refetch it once, not initialize it again"
    );
    assert_eq!(shared.plugin_sessions.len(), 1);
}

#[test]
fn a_page_whose_origin_tab_closed_is_dismissed_instead_of_reopening() {
    let (_temp, shared) = shared_state_with_plugin();
    let doomed_tab = {
        let mut tabs = shared.signals().tabs.get();
        let id = tabs.open(None);
        shared.signals().tabs.set(tabs);
        id
    };
    open_page(&shared, PAGE, doomed_tab);

    let frame_shared = shared.clone();
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build(move |ctx| {
            app_lifecycle::sweep_orphaned_plugin_sessions(&frame_shared);
            rendering::render_page(ctx, &frame_shared);
        });
    step_until(
        &mut harness,
        "the doomed tab's page never initialized",
        |harness| harness.query_by_label("page-layout-call-2").is_some(),
    );

    let mut tabs = shared.signals().tabs.get();
    tabs.close(doomed_tab);
    shared.signals().tabs.set(tabs);
    for _ in 0..10 {
        harness.step();
    }

    assert!(
        !shared.signals().plugin_dialog_state.get().has_open_page(),
        "a page cannot keep dispatching to a tab that no longer exists"
    );
    assert!(shared.plugin_sessions.is_empty());
    assert!(
        harness.query_by_label("page-layout-call-3").is_none(),
        "the tab sweep and page renderer must not reopen the dead slot every frame"
    );
}

#[test]
fn a_failed_page_open_is_terminal_and_visible() {
    let (_temp, shared) = shared_state_with_plugin();
    let tab = active_tab(&shared);
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.open_page("missing-plugin", "missing-page", tab);
    signal.set(state);

    let slot = PluginSlot::Page {
        plugin_id: "missing-plugin".to_string(),
        page_id: "missing-page".to_string(),
        tab,
    };
    let mut harness = page_harness(&shared);
    step_until(
        &mut harness,
        "the failed page never showed a terminal error",
        |_| {
            matches!(
                shared.plugin_sessions.view(
                    shared.facade.as_ref().expect("the fixture has a facade"),
                    shared.services.tokio_runtime.handle(),
                    &slot,
                ),
                SlotView::Failed(_)
            )
        },
    );
    for _ in 0..10 {
        harness.step();
    }

    let SlotView::Failed(error) = shared.plugin_sessions.view(
        shared.facade.as_ref().expect("the fixture has a facade"),
        shared.services.tokio_runtime.handle(),
        &slot,
    ) else {
        panic!("the page failure must stay terminal");
    };
    let error_label = format!("Plugin UI error: {error}");
    assert!(
        harness.query_by_label(&error_label).is_some(),
        "the terminal slot failure must be visible in the page body"
    );
    assert!(
        !shared
            .signals()
            .plugin_dialog_state
            .get()
            .page_init_pending(),
        "a terminal session-open failure must also settle page initialization"
    );
    assert_eq!(shared.plugin_sessions.len(), 1);
}
