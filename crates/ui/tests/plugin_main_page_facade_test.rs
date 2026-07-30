//! End-to-end coverage for the plugin detail view's `MainPage`, against a
//! real `ArclainApp` and a real, running WASM plugin.
//!
//! `plugin_session_facade_test.rs` drives the session registry directly;
//! `plugin_document_render_test.rs` drives clicks against hand-built
//! documents. These drive the view itself -- `detail_view::render` in a
//! headless egui harness, with the same `PluginsListState` the settings
//! page hands it -- so what is pinned here is the thing neither of those
//! can see: that the plugin's configuration section really is served by a
//! facade session, that a press in it reaches the plugin and its result
//! comes back, and that repeated frames do not re-enter the guest.
//!
//! The last of those is why `facade-test-fixture` is used rather than a
//! nicer-looking fixture: its `MainPage` layout embeds its own
//! `get-ui-layout` call count (`layout-call-{n}`), so the number of times
//! the host asked the plugin what to draw is readable straight off the
//! screen. A regression that re-fetched per frame would show a rising
//! counter rather than merely being slow.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::features::plugins::application::{PluginSlot, SlotView};
use arclain_ui::features::plugins::domain::types::{PluginInfo, PluginStatus, PluginsListState};
use arclain_ui::features::plugins::presentation::views::detail_view;
use arclain_ui::shared::theme::AppTheme;
use arclain_ui::shared::SharedState;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

/// Deterministic layout counter and real host intents -- see this file's
/// own doc comment.
const FIXTURE: &str = "facade-test-fixture";
/// Offers no `MainPage` at all, so it also exercises the empty-document
/// branch of the section.
const DEMO: &str = "ui-demo";

/// Copies a workspace plugin fixture into the folder layout the plugin
/// loader expects -- mirrors `plugin_session_facade_test.rs`'s helper of
/// the same name (each test binary is its own crate).
fn install_plugin_fixture(plugins_dir: &Path, name: &str) {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
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

/// A `SharedState` whose facade is bootstrapped against an isolated temp
/// directory with `plugins` installed and loaded.
///
/// The returned `TempDir` MUST stay alive for the whole test: dropping it
/// deletes the databases the facade has open.
fn shared_state_with_plugins(plugins: &[&str]) -> (tempfile::TempDir, SharedState) {
    let temp = tempfile::tempdir().expect("create tempdir for the test facade");
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    for plugin in plugins {
        install_plugin_fixture(&paths.plugins_dir, plugin);
    }
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with plugin fixtures must succeed");

    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app);
    (temp, shared)
}

fn main_page_slot(plugin_id: &str) -> PluginSlot {
    PluginSlot::MainPage {
        plugin_id: plugin_id.to_string(),
    }
}

fn plugin_info(id: &str) -> PluginInfo {
    PluginInfo {
        id: id.to_string(),
        name: format!("{id} (test)"),
        version: "1.0.0".to_string(),
        author: None,
        description: None,
        capabilities: Vec::new(),
        enabled: true,
        loaded: true,
        status: PluginStatus::Ready,
        error: None,
        visibility: HashMap::new(),
    }
}

struct Stage {
    shared: SharedState,
    state: PluginsListState,
    theme: AppTheme,
}

impl Stage {
    /// Whether the view has opened a facade session for `plugin_id`'s
    /// `MainPage`.
    ///
    /// Deliberately `PluginSessions::session_id` rather than `view`: this
    /// is what every wait below polls, and `view` *declares* an absent
    /// slot. Polling with it would open the session the render is
    /// supposed to open, and every test here would still pass with the
    /// cutover removed.
    fn session_open(&self, plugin_id: &str) -> bool {
        self.shared
            .plugin_sessions
            .session_id(&main_page_slot(plugin_id))
            .is_some()
    }

    /// The document the slot holds. Only meaningful once
    /// [`Stage::session_open`] is true -- `view` opens an absent slot, so
    /// calling this first would be the same trap.
    fn document(
        &self,
        plugin_id: &str,
    ) -> Option<std::sync::Arc<arclain_app::plugins::PluginUiDocument>> {
        let facade = self.shared.facade.as_ref()?;
        match self.shared.plugin_sessions.view(
            facade,
            self.shared.services.tokio_runtime.handle(),
            &main_page_slot(plugin_id),
        ) {
            SlotView::Ready(document) => Some(document),
            SlotView::Opening | SlotView::Failed(_) => None,
        }
    }
}

fn detail_harness(shared: SharedState, plugins: &[&str]) -> Harness<'static, Stage> {
    let state = PluginsListState {
        plugins: plugins.iter().copied().map(plugin_info).collect(),
        selected_plugin: plugins.first().map(|id| (*id).to_string()),
        ..PluginsListState::default()
    };
    let theme = shared.theme.clone();
    Harness::new_ui_state(
        |ui, stage: &mut Stage| {
            detail_view::render(ui, &stage.theme, &mut stage.state, Some(&stage.shared));
        },
        Stage {
            shared,
            state,
            theme,
        },
    )
}

/// Drives frames the way a running application does until `ready`, rather
/// than sleeping on a guess: the session open is spawned by the first
/// frame and completes on the runtime, so only a later frame can observe
/// it.
///
/// Settles with `run_ok` rather than `run` on purpose. This view keeps
/// asking for repaints in states these tests deliberately pass through
/// (the `Opening` spinner animates, and so does a toast an applied intent
/// raised), and the harness's `run` panics rather than returning once a
/// repaint is still pending after its step budget. "The UI never stops
/// animating" is not the property under test here; the assertions below
/// are.
fn run_until(harness: &mut Harness<'static, Stage>, what: &str, ready: impl Fn(&Stage) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        harness.step();
        if ready(harness.state()) {
            settle(harness);
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs frames until the UI stops asking for immediate repaints, or until
/// the harness's step budget runs out -- see [`run_until`].
fn settle(harness: &mut Harness<'static, Stage>) {
    let _ = harness.run_ok();
}

fn opened(plugin_id: &'static str) -> impl Fn(&Stage) -> bool {
    move |stage: &Stage| stage.session_open(plugin_id)
}

/// The cutover itself: the plugin's configuration section is the
/// plugin's real `MainPage` document, fetched through a facade session.
#[test]
fn the_detail_view_draws_the_selected_plugins_main_page_from_a_facade_document() {
    let (_temp, shared) = shared_state_with_plugins(&[FIXTURE]);
    let mut harness = detail_harness(shared, &[FIXTURE]);

    run_until(
        &mut harness,
        "the MainPage session to open",
        opened(FIXTURE),
    );

    let stage = harness.state();
    let document = stage.document(FIXTURE).expect("the slot holds a document");
    assert_eq!(document.plugin_id, FIXTURE);
    assert_eq!(document.region_id, "main_page");
    assert!(
        stage
            .shared
            .plugin_sessions
            .session_id(&main_page_slot(FIXTURE))
            .is_some(),
        "the section must be served by a real facade session"
    );

    // The plugin's own layout, on screen -- not a placeholder and not the
    // "no configuration" message.
    assert!(
        harness.query_by_label("layout-call-1").is_some(),
        "the fixture's real MainPage label must be drawn"
    );
    assert!(harness.query_by_label("Multi Action").is_some());
    assert!(harness.query_by_label("Trigger Trap").is_some());
}

/// The other half of the round trip: a press in the rendered document
/// reaches the plugin through `start_plugin_action`, and the result comes
/// back to this slot -- document revision and host intents both.
///
/// Driven through the real operation bridge rather than by applying the
/// update by hand, so what is pinned is the path the application actually
/// runs.
#[test]
fn a_main_page_press_dispatches_through_the_facade_and_applies_its_result() {
    let (_temp, shared) = shared_state_with_plugins(&[FIXTURE]);
    arclain_ui::core::operation_bridge::spawn(&shared);
    let mut harness = detail_harness(shared, &[FIXTURE]);

    run_until(
        &mut harness,
        "the MainPage session to open",
        opened(FIXTURE),
    );
    let opened_revision = harness
        .state()
        .document(FIXTURE)
        .expect("the slot holds a document")
        .revision;

    harness.get_by_label("Multi Action").click();
    settle(&mut harness);

    // `SetPageDisplayName` is the last of the three intents the fixture
    // emits, so observing it means the whole ordered list was applied.
    run_until(
        &mut harness,
        "the action's intents to reach the host",
        |stage| {
            stage
                .shared
                .signals()
                .tabs
                .get()
                .active()
                .page_display_name
                .get()
                .as_deref()
                == Some("third")
        },
    );

    let stage = harness.state();
    assert!(
        stage
            .document(FIXTURE)
            .expect("the slot still holds a document")
            .revision
            > opened_revision,
        "a dispatched action must advance the slot's document revision"
    );
    assert!(
        stage.shared.plugin_sessions.tracked_ids().is_empty(),
        "the completed operation must be drained rather than tracked forever"
    );
    // The fixture's handler emits no refresh, so the document tree is
    // unchanged -- the counter proves the dispatch did not re-read the
    // layout behind the revision bump either.
    assert!(harness.query_by_label("layout-call-1").is_some());
}

/// The cache decision, half one: an unchanged plugin never re-fetches.
///
/// This replaces the old `cached_main_layout` unit test and is strictly
/// stronger than it was. The layout counter is the *plugin's* own, so ten
/// frames showing `layout-call-1` prove the host asked the guest exactly
/// once -- the audited defect this section used to carry (one WASM
/// `get-ui-layout` per plugin per frame) cannot come back silently.
#[test]
fn re_rendering_the_same_plugin_never_refetches_its_main_page() {
    let (_temp, shared) = shared_state_with_plugins(&[FIXTURE]);
    let mut harness = detail_harness(shared, &[FIXTURE]);

    run_until(
        &mut harness,
        "the MainPage session to open",
        opened(FIXTURE),
    );
    let session_id = harness
        .state()
        .shared
        .plugin_sessions
        .session_id(&main_page_slot(FIXTURE))
        .expect("session opened");

    for _ in 0..10 {
        harness.step();
    }

    assert!(
        harness.query_by_label("layout-call-1").is_some(),
        "a re-fetch would have advanced the fixture's own layout counter"
    );
    assert_eq!(
        harness
            .state()
            .shared
            .plugin_sessions
            .session_id(&main_page_slot(FIXTURE)),
        Some(session_id),
        "repeated frames must reuse the slot's session rather than opening another"
    );
    assert_eq!(harness.state().shared.plugin_sessions.len(), 1);
}

/// The cache decision, half two: selecting another plugin shows that
/// plugin's own `MainPage` and never the previous one's.
///
/// Under the session model this is not an invalidation at all -- the
/// plugin id is part of the slot key, so a selection change asks for a
/// different slot. Returning to the first plugin then re-draws its
/// retained document without re-entering the guest, which is what the
/// window-scoped slot buys over the per-selection cache it replaced.
#[test]
fn selecting_another_plugin_draws_that_plugins_own_main_page() {
    let (_temp, shared) = shared_state_with_plugins(&[FIXTURE, DEMO]);
    let mut harness = detail_harness(shared, &[FIXTURE, DEMO]);

    run_until(
        &mut harness,
        "the MainPage session to open",
        opened(FIXTURE),
    );
    assert!(harness.query_by_label("layout-call-1").is_some());

    harness.state_mut().state.selected_plugin = Some(DEMO.to_string());
    run_until(
        &mut harness,
        "the second plugin's MainPage session to open",
        opened(DEMO),
    );

    assert!(
        harness.query_by_label("layout-call-1").is_none(),
        "the previous plugin's document must not survive a selection change"
    );
    assert!(harness.query_by_label("Multi Action").is_none());
    assert!(
        harness
            .query_by_label("This plugin does not provide configuration.")
            .is_some(),
        "a plugin that offers no MainPage must say so rather than draw an empty section"
    );

    harness.state_mut().state.selected_plugin = Some(FIXTURE.to_string());
    settle(&mut harness);

    assert!(
        harness.query_by_label("layout-call-1").is_some(),
        "returning must re-draw the retained slot's document, not fetch a second layout"
    );
    assert_eq!(
        harness.state().shared.plugin_sessions.len(),
        2,
        "one window-scoped slot per plugin whose settings were opened"
    );
}
