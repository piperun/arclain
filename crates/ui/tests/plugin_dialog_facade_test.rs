//! The plugin dialog, end to end on the facade session path.
//!
//! `plugin_session_facade_test.rs` covers the session registry itself and
//! `plugin_document_render_test.rs` covers the document renderer; this
//! file covers the surface those two meet on for `Dialog` -- the real
//! `render_dialog` a running application calls every frame, drawing a
//! real plugin's real dialog, a press round-tripping through
//! `start_plugin_action`, every route a dialog can close by releasing its
//! session, and the navigation bridge to the page half that has not been
//! cut over yet.
//!
//! Uses `facade-test-fixture`, the one workspace plugin that implements
//! the `Dialog` extension point (`fixture-dialog`, whose layout carries
//! its own `get-ui-layout` call counter). `ui-demo` implements no dialog
//! at all.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use arclain_app::event::OperationKind;
use arclain_app::ids::PluginSessionId;
use arclain_app::plugins::{
    PluginExtensionPointDto, PluginUiDocument, PluginUiNodeDto, PluginUiNodeKind,
};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_plugins::types::PluginAction;
use arclain_ui::core::app_lifecycle;
use arclain_ui::core::operation_bridge;
use arclain_ui::core::services::Services;
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{PluginSlot, PluginUiJobs};
use arclain_ui::features::plugins::presentation::views::rendering;
use arclain_ui::shared::SharedState;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;
use tempfile::TempDir;

const PLUGIN: &str = "facade-test-fixture";
const DIALOG: &str = "fixture-dialog";
const FIRST_LAYOUT_LABEL: &str = "dialog-layout-call-1";
const ACTION_BUTTON: &str = "Dialog Multi Action";
const CLOSE_BUTTON: &str = "Dialog Close";
const OPEN_PAGE_BUTTON: &str = "Dialog Open Page";
/// The last of the three actions `"multi-action"` returns, in emission
/// order -- so observing it means all three applied, in order.
const LAST_INTENT_VALUE: &str = "third";

// ============================================================================
// Fixture scaffolding.
// ============================================================================

/// Copies a workspace plugin fixture into the folder layout the plugin
/// loader expects -- mirrors `plugin_toolbar_facade_test.rs`'s helper of
/// the same name (each test binary is its own crate).
fn install_plugin_fixture(plugins_dir: &std::path::Path, name: &str) {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name);
    let dest = plugins_dir.join(name);
    std::fs::create_dir_all(&dest).expect("create plugin fixture directory");
    for extension in ["wasm", "toml"] {
        std::fs::copy(
            fixture_dir.join(format!("{name}.{extension}")),
            dest.join(format!("{name}.{extension}")),
        )
        .unwrap_or_else(|error| panic!("copy {name}.{extension} fixture: {error}"));
    }
}

/// A `SharedState` with a real application behind it *and* a real plugin
/// loaded, composed the way `core::state::init` composes the running app:
/// the frontend's legacy service handles come from the application's own
/// composition, so `plugin_ui_jobs` and the facade see one plugin manager
/// rather than two. The page half of this file's bridge test depends on
/// exactly that -- it reads through the legacy queue.
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
    install_plugin_fixture(&paths.plugins_dir, PLUGIN);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap the test facade");
    let legacy = app
        .take_legacy_composition()
        .expect("take the application's own composition");

    let mut shared = common::create_test_shared_state();
    let services = Services {
        core: (*legacy.core_services).clone(),
        plugin_manager: legacy.plugin_manager,
    };
    shared.plugin_ui_jobs = PluginUiJobs::new(
        services.plugin_manager.clone(),
        services.tokio_runtime.clone(),
    );
    shared.services = Arc::new(services);
    shared
        .app_state
        .lock()
        .reload_ui_config(&app, &shared.services.tokio_runtime);
    shared.facade = Some(app);
    (temp, shared)
}

fn active_tab(shared: &SharedState) -> TabId {
    shared.signals().tabs.get().active_id()
}

fn dialog_slot(shared: &SharedState, dialog_id: &str) -> PluginSlot {
    PluginSlot::Dialog {
        plugin_id: PLUGIN.to_string(),
        dialog_id: dialog_id.to_string(),
        tab: active_tab(shared),
    }
}

/// Opens a dialog the way `document_dispatch::apply_navigation` does when
/// a button in some other surface asks for one.
fn open_dialog(shared: &SharedState, dialog_id: &str) {
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.open_dialog(PLUGIN, dialog_id, active_tab(shared));
    signal.set(state);
}

fn dialog_is_open(shared: &SharedState) -> bool {
    shared.signals().plugin_dialog_state.get().has_open_dialog()
}

/// The production dialog frame: the same `render_dialog`
/// `core::arclain_app::dialog_handler` calls once per frame.
fn dialog_harness(shared: &SharedState) -> Harness<'static> {
    let shared = shared.clone();
    Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build(move |ctx| rendering::render_dialog(ctx, &shared))
}

/// Runs frames until `condition` holds.
///
/// The wait is polled through `PluginSessions::session_id` rather than
/// `view`, deliberately: `view` *declares* a slot it does not find, so a
/// test that polled with it would open the very session the render under
/// test is supposed to open -- and keep passing with the cutover reverted.
fn step_until(harness: &mut Harness<'static>, what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        harness.step();
        if condition() {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Draws frames until the open dialog's session exists, and returns it.
fn step_until_session_open(
    harness: &mut Harness<'static>,
    shared: &SharedState,
) -> PluginSessionId {
    let slot = dialog_slot(shared, DIALOG);
    let sessions = shared.plugin_sessions.clone();
    let probe = slot.clone();
    step_until(
        harness,
        "the dialog's plugin session never opened through the facade",
        move || sessions.session_id(&probe).is_some(),
    );
    // The session can complete after the frame that still drew the
    // loading placeholder. Let egui settle the newly sized document
    // window before tests address its AccessKit nodes by their rects.
    harness.run();
    shared
        .plugin_sessions
        .session_id(&slot)
        .expect("the session just observed")
}

fn document(shared: &SharedState, session_id: PluginSessionId) -> PluginUiDocument {
    let facade = shared.facade.as_ref().expect("the fixture has a facade");
    shared
        .services
        .tokio_runtime
        .block_on(facade.plugin_ui_document(session_id))
        .expect("the open session has a document")
}

/// Routes terminal plugin-action events into the session registry the way
/// `core::operation_bridge`'s subscriber does in a running app. Without
/// it the only delivery path is the re-read `document_dispatch::
/// dispatch_action` performs right after registering, and whether that
/// re-read wins the race against the action's own worker is genuinely
/// nondeterministic.
fn spawn_plugin_action_bridge(shared: &SharedState) {
    let facade = shared.facade.clone().expect("the fixture has a facade");
    let mut events = facade.subscribe_operations();
    let shared = shared.clone();
    shared.services.tokio_runtime.clone().spawn(async move {
        while let Ok(event) = events.recv().await {
            if event.kind == OperationKind::PluginAction {
                operation_bridge::handle_plugin_action_event(&shared, event);
            }
        }
    });
}

// ============================================================================
// The dialog draws what the plugin's document says.
// ============================================================================

#[test]
fn the_dialog_draws_its_plugins_document_from_a_facade_session() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    let session_id = step_until_session_open(&mut harness, &shared);
    harness.step();

    let document = document(&shared, session_id);
    assert_eq!(
        document.extension_point,
        PluginExtensionPointDto::Dialog(DIALOG.to_string()),
        "the slot must open the dialog extension point the open-dialog entry names"
    );
    assert_eq!(document.region_id, format!("dialog:{DIALOG}"));
    assert!(
        harness.query_by_label(FIRST_LAYOUT_LABEL).is_some(),
        "the dialog must draw the plugin's own layout, not a placeholder"
    );
    assert!(harness.query_by_label(ACTION_BUTTON).is_some());
}

/// The audited defect the session model removes: the pre-cutover renderer
/// asked the worker for a layout every frame the dialog was open. A slot
/// re-enters the guest only when an action dispatched against it returns,
/// and the fixture's own call counter is the witness.
#[test]
fn re_rendering_an_open_dialog_never_refetches_its_layout() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    let session_id = step_until_session_open(&mut harness, &shared);
    for _ in 0..10 {
        harness.step();
    }

    assert!(
        harness.query_by_label(FIRST_LAYOUT_LABEL).is_some(),
        "ten frames must not have re-entered the plugin's get-ui-layout"
    );
    assert_eq!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, DIALOG)),
        Some(session_id),
        "and they must not have re-opened the session either"
    );
}

// ============================================================================
// A press reaches the plugin, and the result lands on the slot.
// ============================================================================

#[test]
fn a_dialog_press_dispatches_through_the_facade_and_applies_its_result() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);
    spawn_plugin_action_bridge(&shared);

    let mut harness = dialog_harness(&shared);
    let session_id = step_until_session_open(&mut harness, &shared);
    let opened_revision = document(&shared, session_id).revision;

    harness.get_by_label(ACTION_BUTTON).click();
    let origin_tab = active_tab(&shared);
    let display_name = shared
        .signals()
        .tabs
        .get()
        .get(origin_tab)
        .expect("the active tab exists")
        .page_display_name
        .clone();
    step_until(
        &mut harness,
        "the dialog press never round-tripped through the plugin",
        move || display_name.get().as_deref() == Some(LAST_INTENT_VALUE),
    );

    assert!(
        document(&shared, session_id).revision > opened_revision,
        "the press must advance the session's document revision"
    );
    assert_eq!(
        shared.plugin_sessions.tracked_ids(),
        Vec::new(),
        "the operation must be drained rather than tracked forever"
    );
}

// ============================================================================
// Every route a dialog closes by releases its session.
// ============================================================================

/// Route 1: the window's own close button.
#[test]
fn closing_the_dialog_window_releases_its_session() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    step_until_session_open(&mut harness, &shared);

    harness.get_by_label("Close window").click();
    harness.step();
    assert!(!dialog_is_open(&shared), "the X must close the dialog");
    // The reconcile owns the session close, so it lands on the next frame.
    harness.step();

    assert_eq!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, DIALOG)),
        None
    );
    assert!(
        shared.plugin_sessions.is_empty(),
        "no slot may outlive its dialog"
    );
}

/// Route 2: a `CloseDialog` button *inside* the dialog, resolved by the
/// renderer from the node's own typed action.
///
/// The fixture has no `on-ui-event` arm for `"dialog-close"`, so if the
/// press had been forwarded to the guest as a plugin interaction (the
/// pre-cutover reserved-event-id encoding) the dialog would simply stay
/// open -- which is what this asserts against.
#[test]
fn a_close_dialog_button_releases_the_session_without_reaching_the_plugin() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    step_until_session_open(&mut harness, &shared);

    harness.get_by_label(CLOSE_BUTTON).click();
    harness.step();
    assert!(
        !dialog_is_open(&shared),
        "a CloseDialog button must be resolved by the host, not sent to the plugin"
    );
    harness.step();

    assert_eq!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, DIALOG)),
        None
    );
    assert!(shared.plugin_sessions.is_empty());
}

/// Route 3: the legacy job queue's `PluginAction::CloseDialog`.
///
/// Reachable for as long as `Page` renders through `PluginUiJobs`: a page
/// event's returned actions come back through
/// `application::process_actions_for_origin`, which applies them to the
/// shared `PluginDialogState` and writes it back -- exactly what this
/// test does by hand. Nothing on that path knows the facade session
/// registry exists, which is why the close is reconciled from the state
/// rather than hooked into each site.
#[test]
fn a_legacy_close_dialog_action_releases_the_session() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    step_until_session_open(&mut harness, &shared);

    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    let mut toaster = arclain_widgets::Toaster::new();
    arclain_ui::features::plugins::presentation::controllers::plugin_controller::process_plugin_actions(
        vec![PluginAction::CloseDialog],
        PLUGIN,
        &mut state,
        &mut toaster,
        None,
        None,
        Some(&shared),
    );
    signal.set(state);
    assert!(
        !dialog_is_open(&shared),
        "precondition: the legacy action cleared the entry"
    );

    harness.step();

    assert_eq!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, DIALOG)),
        None
    );
    assert!(
        shared.plugin_sessions.is_empty(),
        "a close route that never touches the registry must still release the session"
    );
}

/// Route 4: replacement rather than closure. A second dialog is a
/// different slot key, so the first one's session has no host left.
#[test]
fn opening_a_second_dialog_releases_the_first_ones_session() {
    let (_temp, shared) = shared_state_with_plugin();
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    let first = step_until_session_open(&mut harness, &shared);

    open_dialog(&shared, "another-dialog");
    harness.step();

    assert_eq!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, DIALOG)),
        None,
        "the replaced dialog's session must not survive"
    );
    assert_ne!(
        shared
            .plugin_sessions
            .session_id(&dialog_slot(&shared, "another-dialog")),
        Some(first),
        "and the new dialog must not inherit it"
    );
    assert_eq!(
        shared.plugin_sessions.len(),
        1,
        "exactly one dialog slot may exist at a time"
    );
}

/// Route 5: the dialog's origin tab closes.
///
/// A `Dialog` slot is tab-scoped, so `sweep_orphaned_plugin_sessions`
/// closes it the moment the tab goes -- but nothing on the five tab-close
/// paths clears the navigation entry naming that tab, so the renderer
/// would re-open the session at the bottom of the very same frame the
/// sweep closed it at the top of. This drives both reconciles in the
/// order `update_app` runs them, and asserts the loop does not happen:
/// the fixture's own layout counter is the witness, since every re-open
/// re-enters `get-ui-layout`.
#[test]
fn a_dialog_whose_tab_closed_is_dismissed_instead_of_reopening_every_frame() {
    let (_temp, shared) = shared_state_with_plugin();
    let doomed_tab = {
        let mut tabs = shared.signals().tabs.get();
        let id = tabs.open(None);
        shared.signals().tabs.set(tabs);
        id
    };
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.open_dialog(PLUGIN, DIALOG, doomed_tab);
    signal.set(state);
    let slot = PluginSlot::Dialog {
        plugin_id: PLUGIN.to_string(),
        dialog_id: DIALOG.to_string(),
        tab: doomed_tab,
    };

    // The two per-frame reconciles, in the order the real frame runs
    // them: the tab sweep first, the dialog renderer last.
    let frame_shared = shared.clone();
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build(move |ctx| {
            app_lifecycle::sweep_orphaned_plugin_sessions(&frame_shared);
            rendering::render_dialog(ctx, &frame_shared);
        });
    let sessions = shared.plugin_sessions.clone();
    let probe = slot.clone();
    step_until(
        &mut harness,
        "the dialog's session never opened while its tab was still open",
        move || sessions.session_id(&probe).is_some(),
    );
    assert!(harness.query_by_label(FIRST_LAYOUT_LABEL).is_some());

    let mut tabs = shared.signals().tabs.get();
    tabs.close(doomed_tab);
    shared.signals().tabs.set(tabs);
    assert!(
        shared.signals().tabs.get().get(doomed_tab).is_none(),
        "precondition: the origin tab is gone"
    );
    for _ in 0..10 {
        harness.step();
    }

    assert!(
        !dialog_is_open(&shared),
        "a dialog whose origin tab closed must be dismissed, not left dispatching to a dead tab"
    );
    assert_eq!(shared.plugin_sessions.session_id(&slot), None);
    assert!(shared.plugin_sessions.is_empty());
    assert!(
        harness.query_by_label("dialog-layout-call-2").is_none(),
        "the sweep and the renderer must not fight over the slot once per frame"
    );
}

// ============================================================================
// The bridge to the page half, which has not been cut over.
// ============================================================================

/// A dialog button asking for a page writes the *shared*
/// `PluginDialogState` page stack, which the still-legacy `render_page`
/// reads -- so a facade-rendered dialog can navigate into a legacy page
/// during the interim.
///
/// Asserts both halves: that the navigation is applied to the shared
/// state (including the page-init generation the legacy renderer gates
/// its first layout read on), and that the legacy renderer then actually
/// draws that page.
#[test]
fn a_dialog_button_opens_a_page_the_legacy_renderer_draws() {
    let (_temp, shared) = shared_state_with_plugin();
    let origin_tab = active_tab(&shared);
    open_dialog(&shared, DIALOG);

    let mut harness = dialog_harness(&shared);
    step_until_session_open(&mut harness, &shared);
    harness.get_by_label(OPEN_PAGE_BUTTON).click();
    harness.step();

    let state = shared.signals().plugin_dialog_state.get();
    assert_eq!(
        state.current_page(),
        Some((PLUGIN, "fixture-page", origin_tab)),
        "the dialog's OpenPage must push onto the page stack the legacy renderer reads"
    );
    assert!(
        state.page_init_pending(),
        "and must arm the page-init generation that renderer gates its first layout read on"
    );
    drop(state);

    let page_shared = shared.clone();
    let mut page_harness = Harness::builder()
        .with_size(eframe::egui::vec2(900.0, 700.0))
        .build(move |ctx| {
            assert!(
                rendering::render_page(ctx, &page_shared),
                "the legacy page renderer must claim the content area"
            );
        });
    page_harness.step();

    assert!(
        page_harness.query_by_label("fixture-page").is_some(),
        "the legacy page renderer must draw the page the facade dialog opened"
    );
}

// ============================================================================
// The host's own layout choice.
// ============================================================================

/// A dialog owns its window, so a `Split` document is drawn at
/// `DocumentExtent::Full` -- a real two-pane layout with the sidebar left
/// of the content, rather than the two lists drawn one after the other
/// that the pre-cutover dialog path produced.
///
/// Hand-built rather than fixture-driven: no plugin in this workspace
/// returns a `Split` dialog, and the property under test is the host's,
/// not the plugin's.
#[test]
fn a_split_dialog_document_is_drawn_as_two_panes() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let origin_tab = active_tab(&shared);
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.open_dialog("demo", DIALOG, origin_tab);
    signal.set(state);

    let slot = PluginSlot::Dialog {
        plugin_id: "demo".to_string(),
        dialog_id: DIALOG.to_string(),
        tab: origin_tab,
    };
    shared.plugin_sessions.adopt_for_test(
        &slot,
        PluginSessionId::from_raw(1),
        split_document(PluginSessionId::from_raw(1)),
    );

    let mut harness = dialog_harness(&shared);
    harness.step();

    let sidebar = harness.get_by_label("sidebar-pane").rect();
    let content = harness.get_by_label("content-pane").rect();
    assert!(
        sidebar.right() <= content.left(),
        "a Split dialog must render side by side, got sidebar {sidebar:?} content {content:?}"
    );
}

fn split_document(session_id: PluginSessionId) -> PluginUiDocument {
    fn label(id: &str, text: &str) -> PluginUiNodeDto {
        PluginUiNodeDto {
            id: id.to_string(),
            kind: PluginUiNodeKind::Label {
                text: text.to_string(),
                bold: false,
                size: None,
            },
            visible: true,
            enabled: true,
        }
    }

    PluginUiDocument {
        session_id,
        plugin_id: "demo".to_string(),
        region_id: format!("dialog:{DIALOG}"),
        extension_point: PluginExtensionPointDto::Dialog(DIALOG.to_string()),
        revision: 1,
        root: PluginUiNodeDto {
            id: "#root".to_string(),
            kind: PluginUiNodeKind::Split {
                sidebar: vec![label("side", "sidebar-pane")],
                content: vec![label("main", "content-pane")],
                sidebar_width: Some(120.0),
            },
            visible: true,
            enabled: true,
        },
    }
}
