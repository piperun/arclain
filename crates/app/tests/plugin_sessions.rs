//! Integration tests for renderer-neutral plugin sessions: `plugins`,
//! `set_plugin_enabled`, `open_plugin_session`, `plugin_ui_document`,
//! `close_plugin_session`, `start_plugin_action`, and
//! `set_active_archive_session` -- all driven through `ArclainApp`'s
//! public facade against the real bundled `ui-demo` WASM fixture, the
//! same way a real frontend would.
//!
//! Plus the plugin-*management* surface beside them: `install_plugin`,
//! the widened `PluginSummary`, and the `plugin_chrome`/
//! `plugin_network_log` read models. Those need a live guest to register
//! a top tab and write a log line; the mirror-fidelity half of the same
//! surface is unit-tested inside `arclain_app::plugins`.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! for the same reason `crates/app/tests/archive_sessions.rs` uses that
//! pattern: `ArclainApp` owns its own Tokio runtime, and dropping it from
//! inside an async context panics. Each test builds `app` in sync code,
//! drives facade calls through one foreign `Runtime::block_on`, and lets
//! `app` drop only after `block_on` returns.

mod support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::{ArchiveSessionId, PluginSessionId};
// Every plugin-UI DTO type is reached through `arclain_app::plugins`, not
// `arclain_plugins` directly -- proving the facade re-exports the full
// transitive surface `PluginUiDocument`/`PluginActionRequest` expose (see
// that module's own doc comment). `PluginLayout`/`PluginUiElement`/
// `normalize_layout` are the one deliberate exception used later in this
// file: they are pre-normalization, WIT-facing types nothing outside
// `arclain_plugins` ever receives from the real facade, only useful here
// to hand-build a sample document for one serde round-trip test.
use arclain_app::plugins::{
    is_plugin_disabled_refusal, PluginActionDto, PluginActionRequest, PluginBadgeDto,
    PluginCapabilityDto, PluginExtensionPointDto, PluginHostIntentDto, PluginToastLevelDto,
    PluginTopTabDto, PluginUiDocument,
};
use arclain_app::{ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

/// The workspace's built `plugins/{name}/{name}.wasm` (produced by `just
/// plugins`) -- the real component `ArclainApp::install_plugin` is pointed
/// at, and the source half of [`install_plugin_fixture`]'s folder copy.
fn fixture_wasm_path(name: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name)
        .join(format!("{name}.wasm"))
}

/// Copies a workspace plugin fixture (`plugins/{name}/{name}.toml`,
/// `{name}.wasm`, built by `just plugins`) into `plugins_dir/{name}/`,
/// the folder-mode layout `arclain_plugins::loader::PluginLoader::
/// discover_plugins` expects. Exercising a real, running plugin instance
/// (rather than a hand-built `arclain_plugins::types::PluginLayout`)
/// proves the whole path end to end: WASM `get-ui-layout`/`on-ui-event`
/// calls, normalization, and the facade session/action wiring together.
///
/// Not the same thing as [`ArclainApp::install_plugin`], which derives its
/// manifest from the component's own metadata export rather than from the
/// hand-written `.toml` this copies. Both paths are covered.
fn install_plugin_fixture(plugins_dir: &std::path::Path, name: &str) {
    let dest_dir = plugins_dir.join(name);
    let fixture_dir = fixture_wasm_path(name)
        .parent()
        .expect("a fixture .wasm always has a parent directory")
        .to_path_buf();
    std::fs::create_dir_all(&dest_dir).expect("create plugin fixture directory");
    std::fs::copy(
        fixture_dir.join(format!("{name}.wasm")),
        dest_dir.join(format!("{name}.wasm")),
    )
    .expect("copy plugin fixture .wasm");
    std::fs::copy(
        fixture_dir.join(format!("{name}.toml")),
        dest_dir.join(format!("{name}.toml")),
    )
    .expect("copy plugin fixture .toml");
}

/// Bootstraps an `ArclainApp` against an isolated temp profile with the
/// named plugin fixture installed and a working (dummy-path) 7-Zip --
/// see `archive_sessions.rs::bootstrap_app`'s identical rationale for the
/// 7-Zip seed.
fn bootstrap_app_with_plugin(temp: &tempfile::TempDir, plugin_name: &str) -> ArclainApp {
    bootstrap_app_with_plugins(temp, &[plugin_name])
}

/// [`bootstrap_app_with_plugin`] with more than one fixture installed --
/// what the enabled-gate tests need to prove the gate is *per plugin*:
/// disabling one must leave every other plugin's sessions working.
fn bootstrap_app_with_plugins(temp: &tempfile::TempDir, plugin_names: &[&str]) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    for plugin_name in plugin_names {
        install_plugin_fixture(&paths.plugins_dir, plugin_name);
    }
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with the plugin fixture must succeed")
}

fn bootstrap_app_with_ui_demo(temp: &tempfile::TempDir) -> ArclainApp {
    bootstrap_app_with_plugin(temp, "ui-demo")
}

fn bootstrap_app_with_ui_demo_visibility(temp: &tempfile::TempDir, visibility: &str) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_config(&paths, &dummy_sevenzip(temp), Some(visibility.to_string()));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    install_plugin_fixture(&paths.plugins_dir, "ui-demo");
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with persisted plugin visibility")
}

/// Boots a *second* `ArclainApp` over a profile an earlier one already
/// created and populated -- the restart half of a persistence round trip.
///
/// Deliberately seeds nothing. `support::seed_working_sevenzip_config`
/// saves a brand-new `UserConfig` row, so calling it a second time would
/// erase every column the first application wrote, including the very one
/// a persistence test exists to read back. The plugin fixtures and the
/// 7-Zip path are already on disk from the first bootstrap.
fn rebootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(support::temp_paths(temp.path())),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("re-bootstrapping over an existing profile must succeed")
}

/// The same isolated temp profile, with an *empty* plugins directory --
/// the starting point for the `install_plugin` tests, which need the
/// component to arrive through the facade rather than through a folder
/// copy made before bootstrap.
fn bootstrap_app_without_plugins(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with no plugins installed must succeed")
}

#[test]
fn installing_the_active_tab_bridge_wires_existing_plugin_instances() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_from_fallback = observed.clone();

    assert!(
        app.install_active_tab_bridge(move |metadata| {
            observed_from_fallback.lock().unwrap().push(metadata);
        })
        .expect("installing the bridge on a running app must succeed"),
        "the bootstrapped plugin runtime must report that it was wired",
    );

    // The legacy handle is used only as a test probe: the production
    // frontend must not receive or wire PluginManager itself. Reaching
    // the bridge through an instance that already existed before the
    // install proves the application updated the manager and its live
    // instances rather than merely retaining a bridge for future loads.
    let legacy = app
        .take_legacy_composition()
        .expect("the running app must expose its transitional composition");
    let manager = legacy
        .plugin_manager
        .expect("the ui-demo fixture requires a plugin manager");
    let instance = manager
        .lock()
        .get_plugin_instance("ui-demo")
        .expect("the ui-demo instance must be loaded");
    let bridge = instance
        .lock()
        .get_active_tab_bridge()
        .expect("the application must install the bridge on existing instances");
    let metadata = Some(serde_json::json!({"product_id": "RJ123456"}));

    bridge.set_active_tab_metadata(metadata.clone());

    assert_eq!(*observed.lock().unwrap(), vec![metadata]);
}

/// Bootstraps with `facade-test-fixture`, the deterministic plugin built
/// only for this file's own crash-containment/action-ordering/refresh-
/// coalescing tests -- see `plugins/facade-test-fixture/src/lib.rs`'s own
/// doc comment for exactly what it does and why `ui-demo` (whose
/// `on-ui-event` always returns an empty action list) cannot stand in for
/// it.
fn bootstrap_app_with_facade_test_fixture(temp: &tempfile::TempDir) -> ArclainApp {
    bootstrap_app_with_plugin(temp, "facade-test-fixture")
}

/// `async fn`, not a plain helper that calls `tokio::time::timeout`
/// directly at its call site: `Sleep::new_timeout` resolves the current
/// Tokio runtime handle at *construction*, not first poll, so
/// `runtime.block_on(tokio::time::timeout(..))` would try to construct
/// the timeout future as a plain argument expression -- evaluated before
/// `block_on` itself runs, with no ambient runtime at all, and panics
/// ("there is no reactor running"). Wrapping the construction inside an
/// `async fn` body defers it to first poll, which happens only once
/// `block_on` has already established the runtime context. Mirrors
/// `archive_sessions.rs::recv_state`'s identical fix for the identical
/// footgun.
async fn recv_operation_event(
    events: &mut tokio::sync::broadcast::Receiver<arclain_app::event::OperationEvent>,
) -> arclain_app::event::OperationEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("an operation event must arrive within 5s")
        .expect("operation event channel must not close")
}

// 30s, not the 10s `archive_sessions.rs::wait_for_archive_opened` uses:
// every test in this file bootstraps its own `ArclainApp` (its own
// multi-thread Tokio runtime) *and* loads/compiles the real `ui-demo`
// WASM component, so `cargo test --workspace` running this file's tests
// in parallel alongside every other crate's own test binaries can
// transiently starve a single operation's worker task for longer than a
// tighter deadline tolerates -- observed as a flake under full-workspace
// load though never under `cargo test -p arclain_app` alone.
async fn wait_for_plugin_ui_updated(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::plugins::PluginUiUpdate {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = app
            .operation(operation_id)
            .await
            .expect("operation must exist");
        match snapshot.state {
            OperationState::Completed {
                result: OperationResult::PluginUiUpdated { update },
            } => return update,
            OperationState::Failed { error } => {
                panic!("plugin action unexpectedly failed: {error:?}")
            }
            OperationState::Cancelled => panic!("plugin action was unexpectedly cancelled"),
            _ if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("plugin action did not complete within the test deadline"),
        }
    }
}

#[test]
fn plugins_reports_the_installed_fixture_as_enabled_with_no_load_error() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let summaries = runtime
        .block_on(app.plugins())
        .expect("plugins() must succeed");

    let ui_demo = summaries
        .iter()
        .find(|summary| summary.id == "ui-demo")
        .expect("ui-demo must be reported");
    assert!(ui_demo.enabled);
    assert_eq!(ui_demo.load_error, None);
    assert_eq!(ui_demo.name, "UI Demo Plugin");
}

#[test]
fn plugins_reports_the_persisted_visibility_for_each_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo_visibility(
        &temp,
        r#"{"ui-demo":{"toolbar":true,"info_panel":false}}"#,
    );
    let runtime = foreign_runtime();

    let summaries = runtime.block_on(app.plugins()).expect("list plugins");
    let ui_demo = summaries
        .iter()
        .find(|summary| summary.id == "ui-demo")
        .expect("ui-demo must be reported");

    assert_eq!(ui_demo.visibility.get("toolbar"), Some(&true));
    assert_eq!(ui_demo.visibility.get("info_panel"), Some(&false));
}

#[test]
fn set_plugin_enabled_toggles_and_is_reflected_by_plugins() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .expect("disabling a known plugin must succeed");
    let summaries = runtime.block_on(app.plugins()).unwrap();
    assert!(
        !summaries
            .iter()
            .find(|s| s.id == "ui-demo")
            .unwrap()
            .enabled
    );

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), true))
        .expect("re-enabling must succeed");
    let summaries = runtime.block_on(app.plugins()).unwrap();
    assert!(
        summaries
            .iter()
            .find(|s| s.id == "ui-demo")
            .unwrap()
            .enabled
    );
}

#[test]
fn set_plugin_enabled_rejects_an_unknown_plugin_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.set_plugin_enabled("does-not-exist".to_string(), true))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

// ===========================================================================
// Plugin management: the widened summary, install, and the chrome /
// network-log read models.
//
// The DTO mirror-fidelity half of this surface (field-for-field and
// variant-for-variant against the `arclain_plugins` shapes, the untrusted
// text bounds, the install error envelope) is unit-tested inside
// `arclain_app::plugins`; what needs a real, running plugin is here.
// ===========================================================================

#[test]
fn plugins_reports_the_manifest_author_description_and_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();

    let summaries = runtime
        .block_on(app.plugins())
        .expect("plugins() must succeed");

    let fixture = summaries
        .iter()
        .find(|summary| summary.id == "facade-test-fixture")
        .expect("the fixture must be reported");
    assert_eq!(fixture.author, "Arclain Team");
    assert!(
        fixture
            .description
            .starts_with("Deterministic plugin used only by"),
        "description was {:?}",
        fixture.description,
    );
    // Exactly what the fixture's manifest declares -- not everything, and
    // in `to_capabilities`'s own order rather than the manifest's.
    assert_eq!(
        fixture.capabilities,
        vec![
            PluginCapabilityDto::ArchiveMetadataRead,
            PluginCapabilityDto::FileRead,
        ],
    );
}

#[test]
fn plugins_reports_a_plugin_that_declares_no_capabilities_with_an_empty_list() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let summaries = runtime.block_on(app.plugins()).unwrap();

    let ui_demo = summaries
        .iter()
        .find(|summary| summary.id == "ui-demo")
        .expect("ui-demo must be reported");
    assert_eq!(ui_demo.author, "Arclain Team");
    assert!(ui_demo.capabilities.is_empty());
}

#[test]
fn install_plugin_loads_a_wasm_component_and_reports_it_immediately() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_without_plugins(&temp);
    let runtime = foreign_runtime();

    let before = runtime.block_on(app.plugins()).unwrap();
    assert!(before.is_empty(), "the profile must start with no plugins");

    let installed = runtime
        .block_on(app.install_plugin(fixture_wasm_path("facade-test-fixture")))
        .expect("installing the bundled fixture component must succeed");

    assert_eq!(installed, "facade-test-fixture");
    let after = runtime.block_on(app.plugins()).unwrap();
    let fixture = after
        .iter()
        .find(|summary| summary.id == "facade-test-fixture")
        .expect("the freshly installed plugin must be listed without a restart");
    assert!(
        fixture.enabled,
        "an installed plugin is enabled for this run"
    );
    assert_eq!(fixture.load_error, None);
    assert_eq!(fixture.name, "Facade Test Fixture");
    // An install derives its manifest from the component's own metadata
    // export, which declares no capabilities -- unlike the folder-mode
    // fixture, whose hand-written `.toml` does.
    assert!(fixture.capabilities.is_empty());
}

#[test]
fn install_plugin_refuses_a_second_install_of_the_same_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_without_plugins(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.install_plugin(fixture_wasm_path("facade-test-fixture")))
        .expect("the first install must succeed");

    let error = runtime
        .block_on(app.install_plugin(fixture_wasm_path("facade-test-fixture")))
        .expect_err("installing an already-installed plugin must fail");

    assert_eq!(error.kind, ApplicationErrorKind::Plugin);
    assert_eq!(error.field.as_deref(), Some("wasm_path"));
    let diagnostic = error.diagnostic.expect("a diagnostic must be attached");
    assert!(diagnostic.len() <= 4096);
    assert!(
        diagnostic.contains("already installed"),
        "diagnostic was {diagnostic:?}",
    );
}

#[test]
fn install_plugin_rejects_a_file_that_is_not_a_wasm_component() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_without_plugins(&temp);
    let runtime = foreign_runtime();

    let not_wasm = temp.path().join("notes.txt");
    std::fs::write(&not_wasm, b"definitely not a wasm component").unwrap();

    let error = runtime
        .block_on(app.install_plugin(not_wasm))
        .expect_err("a non-wasm file must not install");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(error.field.as_deref(), Some("wasm_path"));
    assert!(runtime.block_on(app.plugins()).unwrap().is_empty());
}

#[test]
fn install_plugin_reports_a_missing_file_without_installing_anything() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_without_plugins(&temp);
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.install_plugin(temp.path().join("nowhere").join("ghost.wasm")))
        .expect_err("a path that names no file must not install");

    assert_eq!(error.kind, ApplicationErrorKind::Plugin);
    let diagnostic = error.diagnostic.expect("a diagnostic must be attached");
    assert!(diagnostic.len() <= 4096);
    assert!(runtime.block_on(app.plugins()).unwrap().is_empty());
}

#[test]
fn plugin_chrome_reports_the_counts_and_the_fixtures_declared_top_tab() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();

    let chrome = runtime
        .block_on(app.plugin_chrome())
        .expect("plugin_chrome() must succeed");

    assert_eq!(chrome.summary.total, 1);
    assert_eq!(chrome.summary.enabled, 1);
    assert_eq!(
        chrome.top_tabs,
        vec![PluginTopTabDto {
            plugin_id: "facade-test-fixture".to_string(),
            id: "fixture-tab".to_string(),
            label: "Fixture".to_string(),
            icon: "DATABASE".to_string(),
            badge: Some(PluginBadgeDto {
                count: Some(7),
                dot: true,
                color: "orange".to_string(),
            }),
            priority: 250,
        }],
    );
}

#[test]
fn plugin_chrome_drops_a_disabled_plugins_tab_but_still_counts_it() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_enabled("facade-test-fixture".to_string(), false))
        .expect("disabling must succeed");

    let chrome = runtime.block_on(app.plugin_chrome()).unwrap();

    assert_eq!(chrome.summary.total, 1, "a disabled plugin is still loaded");
    assert_eq!(chrome.summary.enabled, 0);
    assert!(
        chrome.top_tabs.is_empty(),
        "only enabled plugins contribute tabs",
    );
}

#[test]
fn plugin_chrome_reports_no_tabs_for_a_plugin_that_registers_none() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let chrome = runtime.block_on(app.plugin_chrome()).unwrap();

    assert_eq!(chrome.summary.total, 1);
    assert_eq!(chrome.summary.enabled, 1);
    assert!(chrome.top_tabs.is_empty());
}

#[test]
fn plugin_network_log_reports_the_line_the_fixture_wrote_at_load() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();

    let entries = runtime
        .block_on(app.plugin_network_log())
        .expect("plugin_network_log() must succeed");

    assert_eq!(entries.len(), 1, "entries were {entries:?}");
    assert_eq!(entries[0].message, "facade-test-fixture: initialized");
    assert!(
        entries[0].logged_at_unix_ms > 1_600_000_000_000,
        "a real timestamp, not the epoch: {}",
        entries[0].logged_at_unix_ms,
    );
}

#[test]
fn plugin_network_log_drops_a_disabled_plugins_lines() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_enabled("facade-test-fixture".to_string(), false))
        .expect("disabling must succeed");

    let entries = runtime.block_on(app.plugin_network_log()).unwrap();

    assert!(
        entries.is_empty(),
        "the log aggregates enabled plugins only, got {entries:?}",
    );
}

#[test]
fn plugin_network_log_is_empty_for_a_plugin_that_logs_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let entries = runtime.block_on(app.plugin_network_log()).unwrap();

    assert!(entries.is_empty(), "entries were {entries:?}");
}

#[test]
fn open_plugin_session_normalizes_the_main_page_layout_and_documents_match_the_immediate_query() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .expect("opening a session with a real plugin must succeed");

    assert_eq!(snapshot.document.plugin_id, "ui-demo");
    assert_eq!(snapshot.document.session_id, snapshot.session_id);
    assert_eq!(snapshot.document.revision, 1);
    assert_eq!(snapshot.document.region_id, "main_page");

    let queried = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .expect("immediate document query must succeed for an open session");
    assert_eq!(queried, snapshot.document);
}

#[test]
fn open_plugin_session_rejects_an_unknown_plugin_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.open_plugin_session(
            "does-not-exist".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

/// One test per current WIT extension point kind, per the amended
/// contract's `open_plugin_session(plugin_id, extension_point)` signature
/// -- proving every kind actually opens a session and tags its document
/// with the extension point/region it was asked for. `Panel` and
/// `PluginButton` additionally assert on `ui-demo`'s real, non-empty
/// layout content for that extension point (`MainPage`/`Dialog`/`Page`
/// aren't implemented by the fixture and fall through to an empty
/// layout -- still a real, successfully-opened session, just with no
/// content to assert further on).
#[test]
fn open_plugin_session_opens_the_main_page_extension_point() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .expect("MainPage must open");

    assert_eq!(
        snapshot.document.extension_point,
        PluginExtensionPointDto::MainPage
    );
    assert_eq!(snapshot.document.region_id, "main_page");
}

#[test]
fn open_plugin_session_opens_the_panel_extension_point_with_real_content() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::Panel))
        .expect("Panel must open");

    assert_eq!(
        snapshot.document.extension_point,
        PluginExtensionPointDto::Panel
    );
    assert_eq!(snapshot.document.region_id, "panel");
    let arclain_app::plugins::PluginUiNodeKind::Single { children } = &snapshot.document.root.kind
    else {
        panic!("expected a Single root");
    };
    assert_eq!(children.len(), 2, "ui-demo's Panel layout has two labels");
}

#[test]
fn open_plugin_session_opens_the_plugin_button_extension_point_with_real_content() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(
            app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::PluginButton),
        )
        .expect("PluginButton must open");

    assert_eq!(
        snapshot.document.extension_point,
        PluginExtensionPointDto::PluginButton
    );
    assert_eq!(snapshot.document.region_id, "plugin_button");
    let arclain_app::plugins::PluginUiNodeKind::Single { children } = &snapshot.document.root.kind
    else {
        panic!("expected a Single root");
    };
    assert_eq!(children[0].id, "plugin_toolbar_btn");
}

#[test]
fn open_plugin_session_opens_a_dialog_extension_point() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "ui-demo".to_string(),
            PluginExtensionPointDto::Dialog("confirm".to_string()),
        ))
        .expect("Dialog must open");

    assert_eq!(
        snapshot.document.extension_point,
        PluginExtensionPointDto::Dialog("confirm".to_string())
    );
    assert_eq!(snapshot.document.region_id, "dialog:confirm");
}

#[test]
fn open_plugin_session_opens_a_page_extension_point() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "ui-demo".to_string(),
            PluginExtensionPointDto::Page("settings".to_string()),
        ))
        .expect("Page must open");

    assert_eq!(
        snapshot.document.extension_point,
        PluginExtensionPointDto::Page("settings".to_string())
    );
    assert_eq!(snapshot.document.region_id, "page:settings");
}

/// A page's internal `__page_init` lifecycle event runs after its session
/// has opened, but the frontend must never draw the document fetched
/// before that event. The fixture bakes each `get-ui-layout` call into its
/// first label: opening produces call 1, and page init must make the
/// action's returned document call 2 even though the guest did not emit a
/// `RefreshPanel` action itself.
#[test]
fn page_init_action_returns_a_fresh_post_init_document() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Page("fixture-page".to_string()),
        ))
        .expect("fixture page must open");
    let first_label = |document: &PluginUiDocument| {
        let arclain_app::plugins::PluginUiNodeKind::Single { children } = &document.root.kind
        else {
            panic!("expected a Single root");
        };
        let arclain_app::plugins::PluginUiNodeKind::Label { text, .. } = &children[0].kind else {
            panic!("expected the first child to be a Label");
        };
        text.clone()
    };
    assert_eq!(first_label(&snapshot.document), "page-layout-call-1");

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "__page_init".to_string(),
            action: PluginActionDto::SetValue {
                value: Some("fixture-page".to_string()),
            },
        }))
        .expect("page init action must start");
    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));

    assert_eq!(
        first_label(&update.document),
        "page-layout-call-2",
        "page init must force one post-init layout read before publishing"
    );
    assert_eq!(
        update.intents,
        vec![PluginHostIntentDto::SetPageDisplayName {
            name: "Fixture Page (fixture-page)".to_string(),
        }]
    );
}

#[test]
fn open_plugin_session_rejects_an_empty_or_oversized_dialog_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let empty = runtime
        .block_on(app.open_plugin_session(
            "ui-demo".to_string(),
            PluginExtensionPointDto::Dialog(String::new()),
        ))
        .unwrap_err();
    assert_eq!(empty.kind, ApplicationErrorKind::InvalidInput);

    let oversized = runtime
        .block_on(app.open_plugin_session(
            "ui-demo".to_string(),
            PluginExtensionPointDto::Page("x".repeat(513)),
        ))
        .unwrap_err();
    assert_eq!(oversized.kind, ApplicationErrorKind::InvalidInput);
}

#[test]
fn plugin_ui_document_rejects_an_unknown_session_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.plugin_ui_document(PluginSessionId::from_raw(999_999)))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

#[test]
fn close_plugin_session_makes_it_unreachable_and_closing_twice_is_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .unwrap();

    runtime
        .block_on(app.close_plugin_session(snapshot.session_id))
        .expect("closing an open session must succeed");

    let error = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .unwrap_err();
    assert_eq!(error.kind, ApplicationErrorKind::NotFound);

    let second_close = runtime
        .block_on(app.close_plugin_session(snapshot.session_id))
        .unwrap_err();
    assert_eq!(second_close.kind, ApplicationErrorKind::NotFound);
}

#[test]
fn start_plugin_action_dispatches_through_the_real_plugin_and_completes_with_plugin_ui_updated() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .unwrap();
    let mut events = app.subscribe_operations();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "demo_btn".to_string(),
            action: PluginActionDto::Activate,
        }))
        .expect("start_plugin_action must accept a well-formed request");

    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));
    assert_eq!(update.document.session_id, snapshot.session_id);
    assert_eq!(update.document.revision, 2, "one action, one revision bump");
    // The real ui-demo plugin's `on-ui-event` always returns an empty
    // action list (see plugins/ui-demo/src/lib.rs), so no host intents
    // are expected here -- this proves the *plumbing* end to end
    // (WASM call -> bounded actions -> PluginUiUpdated), while
    // `crates/app/src/plugins.rs`'s own unit tests cover every bounded
    // `PluginAction` -> `PluginHostIntentDto` conversion directly.
    assert!(update.intents.is_empty());

    // The operation-event stream also reports the same terminal result.
    let mut saw_completed = false;
    for _ in 0..32 {
        let event = runtime.block_on(recv_operation_event(&mut events));
        assert_eq!(event.kind, OperationKind::PluginAction);
        if let OperationState::Completed {
            result: OperationResult::PluginUiUpdated { .. },
        } = event.state
        {
            saw_completed = true;
            break;
        }
    }
    assert!(
        saw_completed,
        "subscribe_operations must report the PluginUiUpdated completion"
    );
}

#[test]
fn start_plugin_action_fails_the_operation_for_an_unknown_session() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: PluginSessionId::from_raw(999_999),
            node_id: "does-not-matter".to_string(),
            action: PluginActionDto::Activate,
        }))
        .expect("start_plugin_action itself accepts any well-formed request");

    // Polls through `wait_for_terminal_state` rather than a loop of
    // `runtime.block_on(tokio::time::sleep(..))` calls: `Sleep::
    // new_timeout` resolves the ambient Tokio runtime handle when the
    // future is *constructed*, and a `block_on` argument expression is
    // evaluated before `block_on` establishes that context -- so the
    // sleep panics with "there is no reactor running" (the same footgun
    // `recv_operation_event` documents). That branch only runs when the
    // first poll finds the operation still in flight, which is why it
    // stayed invisible until parallel load made the operation lose the
    // race to the first poll.
    let final_state = runtime.block_on(wait_for_terminal_state(&app, operation_id));

    match final_state {
        OperationState::Failed { error } => assert_eq!(error.kind, ApplicationErrorKind::NotFound),
        other => panic!("expected Failed(NotFound) for an unknown session, got {other:?}"),
    }
}

/// Polls `app.operation(operation_id)` until it reaches any terminal
/// state (`Completed`/`Cancelled`/`Failed`), returning that state as-is --
/// unlike `wait_for_plugin_ui_updated`, this does not itself assert which
/// terminal state was reached, so a caller can assert `Failed` without
/// the helper panicking first. `async fn` for the same
/// construct-inside-`block_on` reason `recv_operation_event` documents.
async fn wait_for_terminal_state(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> OperationState {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = app
            .operation(operation_id)
            .await
            .expect("operation must exist");
        if matches!(
            snapshot.state,
            OperationState::Completed { .. }
                | OperationState::Cancelled
                | OperationState::Failed { .. }
        ) {
            return snapshot.state;
        }
        if std::time::Instant::now() >= deadline {
            panic!("operation did not reach a terminal state within the test deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// **Plugin crash containment** (brief-mandated, previously only argued
/// structurally, not proven). `facade-test-fixture`'s `"trigger-trap"`
/// button panics unconditionally inside `on-ui-event`, which -- under
/// this workspace's `panic = "abort"` plugin release profile -- compiles
/// to a genuine WASM `unreachable` trap.
///
/// Building this fixture surfaced a real finding: wasmtime permanently
/// poisons a *component* instance's `Store` after **any** trap (a second
/// call into the same store fails with wasmtime's own internal "cannot
/// enter component instance", not a fresh attempt) -- there is no sense
/// in which the exact same `Store` becomes call-able again. Before this
/// task, `arclain_plugins::runtime::resource_quota_reason` only
/// classified the two quota-shaped trap variants (`OutOfFuel`/
/// `Interrupt`) as terminal; a genuine guest panic fell through
/// unclassified, so a *second* call would have silently re-attempted the
/// WASM call and surfaced that confusing wasmtime-internal string
/// instead of this crate's own stable, redacted `Unavailable` reason.
/// Fixed alongside this test (see `resource_quota_reason`'s own doc
/// comment) so every trap is classified consistently.
///
/// "Containment" verified here is therefore: the trap never crashes the
/// host process (it surfaces as an ordinary `Result::Err`, caught by
/// this same test process running on); the dispatching operation fails
/// cleanly with a stable `ApplicationErrorKind::Plugin`; the session
/// store and its other bookkeeping are entirely unaffected (the session
/// stays open and queryable); and a *second* interaction with the
/// now-permanently-unavailable plugin instance fails immediately with
/// the same stable, redacted reason -- not a repeat WASM attempt, not a
/// confusing wasmtime-internal string, and not a host crash.
#[test]
fn a_trapping_guest_fails_its_own_operation_without_crashing_the_host_or_the_session_store() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap();

    let trap_operation = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "trigger-trap".to_string(),
            action: PluginActionDto::Activate,
        }))
        .expect("start_plugin_action itself accepts a well-formed request");
    let trap_state = runtime.block_on(wait_for_terminal_state(&app, trap_operation));
    let first_diagnostic = match trap_state {
        OperationState::Failed { error } => {
            assert_eq!(error.kind, arclain_app::error::ApplicationErrorKind::Plugin);
            error.diagnostic
        }
        other => panic!("expected the trapping guest call to fail the operation, got {other:?}"),
    };

    // The session store is entirely unaffected by the trap: a plain,
    // no-WASM-call query still succeeds.
    let queried = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .expect("the session must still be queryable after a guest trap");
    assert_eq!(queried.session_id, snapshot.session_id);

    // A second interaction with the same (now permanently poisoned)
    // plugin instance must still fail cleanly -- host-crash-free, and
    // classified the same stable way, not a repeat WASM attempt that
    // surfaces wasmtime's own raw "cannot enter component instance"
    // string.
    let second_operation = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    let second_state = runtime.block_on(wait_for_terminal_state(&app, second_operation));
    match second_state {
        OperationState::Failed { error } => {
            assert_eq!(error.kind, arclain_app::error::ApplicationErrorKind::Plugin);
            assert_eq!(
                error.diagnostic, first_diagnostic,
                "a second call to a permanently-poisoned instance must be classified the \
                 same stable way as the original trap, not a raw wasmtime-internal string"
            );
        }
        other => panic!("expected the second call to also fail cleanly, got {other:?}"),
    }
}

/// **Action ordering** (brief-mandated, previously only structurally
/// true via a plain `for` loop, never asserted). `facade-test-fixture`'s
/// `"multi-action"` button returns three different actions
/// (`ShowToast`, `CopyToClipboard`, `SetPageDisplayName`) from one
/// `on-ui-event` response; the resulting `PluginUiUpdate::intents` must
/// preserve that exact order.
#[test]
fn multiple_actions_in_one_response_apply_in_order() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));

    assert_eq!(
        update.intents,
        vec![
            PluginHostIntentDto::ShowToast {
                message: "first".to_string(),
                level: PluginToastLevelDto::Info,
            },
            PluginHostIntentDto::CopyToClipboard {
                text: "second".to_string(),
            },
            PluginHostIntentDto::SetPageDisplayName {
                name: "third".to_string(),
            },
        ],
        "intents must preserve the exact order the plugin's single response returned them in"
    );
}

/// **Refresh coalescing** (brief-mandated, previously only a doc-comment
/// claim). `facade-test-fixture`'s `"multi-refresh"` button returns
/// *three* `RefreshPanel` actions from one response; exactly one
/// re-fetch of `get-ui-layout` must follow -- not zero, not three. The
/// fixture bakes its own `get-ui-layout` call count into the returned
/// `MainPage` label's text (`"layout-call-{n}"`), so the resulting
/// document's content proves the count directly rather than relying on
/// the revision number alone (which only proves "one document update",
/// not "one underlying fetch").
#[test]
fn several_refresh_panel_actions_in_one_response_trigger_exactly_one_refetch() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap();
    let label_text = |document: &PluginUiDocument| {
        let arclain_app::plugins::PluginUiNodeKind::Single { children } = &document.root.kind
        else {
            panic!("expected a Single root");
        };
        let arclain_app::plugins::PluginUiNodeKind::Label { text, .. } = &children[0].kind else {
            panic!("expected the first child to be a Label");
        };
        text.clone()
    };
    assert_eq!(
        label_text(&snapshot.document),
        "layout-call-1",
        "opening the session performs the first get-ui-layout call"
    );

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "multi-refresh".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));

    assert_eq!(
        update.document.revision, 2,
        "one dispatched action, one revision bump"
    );
    assert_eq!(
        label_text(&update.document),
        "layout-call-2",
        "three RefreshPanel actions in one response must trigger exactly one re-fetch, \
         not zero (stale label) and not three (layout-call-4)"
    );
}

// ===========================================================================
// Plugin settings written from inside a guest.
//
// A guest's `set-setting` only writes into its own instance and flips a
// dirty bit; nothing in the plugin runtime ever puts that on disk, so the
// host has to pull it after every call that enters a guest. A plugin's
// settings form *is* a plugin UI, which makes an ordinary `on-ui-event`
// the write path a real user takes.
// ===========================================================================

/// Reads `facade-test-fixture`'s `Panel` label, which reports what the
/// guest currently holds for its own remembered-value setting -- the only
/// way a host test can see a value from the guest's side of the boundary.
fn remembered_label(document: &PluginUiDocument) -> String {
    panel_label(document, 0)
}

/// The fixture's second `Panel` label: how many times this plugin has
/// been loaded, maintained by its `init` rather than by a UI event.
fn loads_label(document: &PluginUiDocument) -> String {
    panel_label(document, 1)
}

fn panel_label(document: &PluginUiDocument, index: usize) -> String {
    let arclain_app::plugins::PluginUiNodeKind::Single { children } = &document.root.kind else {
        panic!("expected a Single root");
    };
    let arclain_app::plugins::PluginUiNodeKind::Label { text, .. } = &children[index].kind else {
        panic!("expected child {index} to be a Label");
    };
    text.clone()
}

#[test]
fn a_setting_written_from_a_ui_event_survives_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();
    assert_eq!(remembered_label(&snapshot.document), "remembered:unset");

    // Exactly what a plugin's settings form does: an ordinary UI event
    // whose handler calls `set-setting`.
    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "remember".to_string(),
            action: PluginActionDto::SetValue {
                value: Some("RJ123456".to_string()),
            },
        }))
        .unwrap();
    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));
    assert_eq!(
        remembered_label(&update.document),
        "remembered:RJ123456",
        "the guest holds the value it was just given",
    );

    // The whole point: the value must outlive the process that received
    // it. A second application over the same profile can only report it
    // if the first one pulled it out of the instance and stored it, since
    // a fresh plugin instance starts from whatever the settings seed
    // supplies and nothing else.
    drop(runtime);
    drop(app);
    let restarted = rebootstrap_app(&temp);
    let restarted_runtime = foreign_runtime();
    let reopened = restarted_runtime
        .block_on(restarted.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    assert_eq!(
        remembered_label(&reopened.document),
        "remembered:RJ123456",
        "a setting a plugin wrote must survive a restart",
    );
}

/// `on-ui-event` is not the only guest call a setting can be written
/// from. The fixture's `init` maintains a load counter derived from its
/// own persisted value, so it reaches two only if the first load's write
/// was pulled and stored -- and the only guest entry between that write
/// and the end of the first application's life is the session open.
#[test]
fn a_setting_written_outside_a_ui_event_is_persisted_when_a_session_opens() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let first = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();
    assert_eq!(loads_label(&first.document), "loads:1");

    drop(runtime);
    drop(app);
    let restarted = rebootstrap_app(&temp);
    let restarted_runtime = foreign_runtime();
    let second = restarted_runtime
        .block_on(restarted.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    assert_eq!(
        loads_label(&second.document),
        "loads:2",
        "the second load must see the first load's own write",
    );
}

/// A guest can write a setting and *then* fail. The write lands in
/// host-side instance state, so it outlives the trap -- and losing a
/// user's setting because the plugin misbehaved afterwards would be the
/// worse outcome. Pins the pull on the failure path, which a refactor
/// moving it into the success arm would otherwise break silently.
#[test]
fn a_setting_written_before_a_guest_trap_is_still_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "trigger-trap".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    match runtime.block_on(wait_for_terminal_state(&app, operation_id)) {
        OperationState::Failed { .. } => {}
        other => panic!("the trapping guest must fail its own operation, got {other:?}"),
    }

    drop(runtime);
    drop(app);
    let restarted = rebootstrap_app(&temp);
    let restarted_runtime = foreign_runtime();
    let reopened = restarted_runtime
        .block_on(restarted.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    assert_eq!(
        remembered_label(&reopened.document),
        "remembered:trapped",
        "a setting written before the trap must still be persisted",
    );
}

/// `install_plugin` runs the new plugin's `init` in the guest, and no
/// session is opened afterwards -- so the per-interaction pull never
/// fires and only the exit sweep can save what `init` wrote. Install,
/// shut down, restart: the load counter reaches two only if the sweep
/// ran.
#[test]
fn a_setting_written_at_install_is_persisted_by_the_exit_flush() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_without_plugins(&temp);
    let runtime = foreign_runtime();
    runtime
        .block_on(app.install_plugin(fixture_wasm_path("facade-test-fixture")))
        .expect("installing the fixture must succeed");

    runtime
        .block_on(app.shutdown())
        .expect("shutdown must succeed");
    drop(runtime);
    drop(app);

    let restarted = rebootstrap_app(&temp);
    let restarted_runtime = foreign_runtime();
    let reopened = restarted_runtime
        .block_on(restarted.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    assert_eq!(
        loads_label(&reopened.document),
        "loads:2",
        "the second load must see what the first load's install wrote",
    );
}

/// The other half of the same contract: an interaction that writes no
/// setting must not have one invented for it, and re-reading must not
/// disturb what is stored.
#[test]
fn a_plugin_that_writes_no_setting_keeps_reporting_none() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_facade_test_fixture(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();
    // An interaction that writes nothing, dispatched between two reads.
    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));

    drop(runtime);
    drop(app);
    let restarted = rebootstrap_app(&temp);
    let restarted_runtime = foreign_runtime();
    let reopened = restarted_runtime
        .block_on(restarted.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .unwrap();

    assert_eq!(remembered_label(&reopened.document), "remembered:unset");
}

#[test]
fn set_active_archive_session_accepts_both_a_session_id_and_none() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_active_archive_session(Some(ArchiveSessionId::from_raw(1))))
        .expect("reporting an active session must succeed");
    runtime
        .block_on(app.set_active_archive_session(None))
        .expect("reporting no active session must also succeed");
}

#[test]
fn read_plugin_image_rejects_an_unknown_cache_key_through_the_full_facade() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let error = runtime
        .block_on(app.read_plugin_image("plugin-image:ui-demo:missing".to_string()))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
}

/// The image recovery round trip a renderer actually performs: a document
/// node's image is not cached (read misses), the renderer fetches the
/// node's `url` fallback and stores the bytes back under the *same* key,
/// and the next read returns them byte for byte.
///
/// This is the loop that was structurally impossible before
/// `write_plugin_image` existed: the read decodes the owning plugin out of
/// the key and reads from that plugin's namespace, while the recovery
/// write went to the host's, so the second read missed exactly like the
/// first — a permanently broken image, a 30-second retry forever, and one
/// orphaned host cache entry per attempt. Driven through the real facade
/// so the namespace agreement is proven end to end rather than asserted
/// about two functions in isolation.
#[test]
fn a_missed_plugin_image_can_be_recovered_by_a_write_and_read_back() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    // The exact encoded shape `arclain_app::plugins` rewrites into every
    // image-bearing node at normalization time.
    let key = "plugin-image:ui-demo:cover:RJ000001".to_string();
    let bytes = vec![0x89, 0x50, 0x4E, 0x47, 7, 7, 7, 7];

    runtime.block_on(async {
        assert_eq!(
            app.read_plugin_image(key.clone()).await.unwrap_err().kind,
            ApplicationErrorKind::NotFound,
            "the asset must start uncached, so this proves recovery and not a pre-seeded read"
        );

        app.write_plugin_image(
            "ui-demo".to_string(),
            key.clone(),
            bytes.clone(),
            Some("https://example.invalid/cover.png".to_string()),
        )
        .await
        .expect("the recovery write must succeed");

        assert_eq!(
            app.read_plugin_image(key.clone()).await.unwrap(),
            bytes,
            "the recovery write must land in the namespace the read resolves"
        );
    });
}

/// The bootstrap must build the cache *blob store* under the same root
/// as the cache *index* it was pointed at: `paths_override.cache_dir`.
/// A blob store that silently lands under the OS-conventional cache dir
/// instead splits one profile across two roots: the index references
/// blobs a differently-rooted store does not have, and every *other*
/// profile's bootstrap reconciles (and deletes from) the shared
/// OS-conventional store -- which is also what let concurrently
/// bootstrapping test processes wipe each other's freshly written
/// blobs out from under this file's recovery round trip.
#[test]
fn a_plugin_image_blob_lands_under_the_overridden_cache_dir() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    install_plugin_fixture(&paths.plugins_dir, "ui-demo");
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with an overridden profile must succeed");
    let runtime = foreign_runtime();

    // A payload distinctive enough that finding it under the override
    // root identifies the stored blob rather than an incidental file.
    let bytes: Vec<u8> = (0..64u8).map(|n| n.wrapping_mul(37) ^ 0x5A).collect();
    let key = "plugin-image:ui-demo:cover:ROOTCHECK".to_string();
    runtime
        .block_on(app.write_plugin_image("ui-demo".to_string(), key.clone(), bytes.clone(), None))
        .expect("the write must succeed");
    assert_eq!(
        runtime.block_on(app.read_plugin_image(key)).unwrap(),
        bytes,
        "the write must be readable back"
    );

    assert!(
        some_file_contains(&paths.cache_dir, &bytes),
        "the stored blob must live under the profile's own cache_dir ({}), \
         not the OS-conventional one",
        paths.cache_dir.display()
    );
}

/// Walks `root` looking for any regular file whose contents contain
/// `needle`. The blob store is content-addressed (cacache): the stored
/// object is a file holding the verbatim bytes, so a distinctive
/// payload is findable without depending on the store's layout.
fn some_file_contains(root: &std::path::Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if some_file_contains(&path, needle) {
                return true;
            }
        } else if let Ok(contents) = std::fs::read(&path) {
            if contents
                .windows(needle.len())
                .any(|window| window == needle)
            {
                return true;
            }
        }
    }
    false
}

/// A write is rejected for any key the facade did not itself encode, the
/// same way the read is -- a frontend cannot use this to write into an
/// arbitrary cache namespace of its choosing.
#[test]
fn write_plugin_image_rejects_a_key_the_facade_never_encoded() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    for key in ["cover:RJ000001", "plugin-image-but-not-really", ""] {
        let error = runtime
            .block_on(app.write_plugin_image(
                "ui-demo".to_string(),
                key.to_string(),
                vec![1, 2, 3],
                None,
            ))
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound, "key: {key}");
    }
}

/// The cap is enforced on the way in, not only on the way out. Caching an
/// oversized asset and then rejecting it on every subsequent read would
/// burn disk for something permanently unreadable.
#[test]
fn write_plugin_image_rejects_an_asset_over_the_size_cap() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let key = "plugin-image:ui-demo:huge".to_string();
    let oversized = vec![0u8; arclain_app::plugins::MAX_PLUGIN_IMAGE_BYTES as usize + 1];

    let error = runtime
        .block_on(app.write_plugin_image("ui-demo".to_string(), key.clone(), oversized, None))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    assert_eq!(
        runtime
            .block_on(app.read_plugin_image(key))
            .unwrap_err()
            .kind,
        ApplicationErrorKind::NotFound,
        "a rejected write must leave nothing cached"
    );
}

/// Exhaustive serialization coverage for the `OperationResult` variant
/// this task adds. `ArchiveOpened`/`None` are already covered by earlier
/// tasks' own tests; this only needs to prove `PluginUiUpdated` itself
/// round-trips and does not leak anything unexpected into its JSON shape.
#[test]
fn plugin_ui_updated_operation_result_round_trips_through_serde() {
    let layout = arclain_plugins::types::PluginLayout::Single {
        elements: vec![arclain_plugins::types::PluginUiElement::Label {
            text: "hello".to_string(),
            bold: false,
            size: None,
        }],
    };
    let root = arclain_plugins::ui_model::normalize_layout(&layout).unwrap();
    let document = PluginUiDocument {
        session_id: PluginSessionId::from_raw(1),
        plugin_id: "ui-demo".to_string(),
        region_id: "main_page".to_string(),
        extension_point: PluginExtensionPointDto::MainPage,
        revision: 3,
        root,
    };
    let result = OperationResult::PluginUiUpdated {
        update: arclain_app::plugins::PluginUiUpdate {
            document,
            intents: vec![PluginHostIntentDto::ShowToast {
                message: "done".to_string(),
                level: PluginToastLevelDto::Success,
            }],
        },
    };

    let json = serde_json::to_string(&result).unwrap();
    let restored: OperationResult = serde_json::from_str(&json).unwrap();

    assert_eq!(restored, result);
    assert!(json.contains("\"type\":\"plugin_ui_updated\""));
}

// ===========================================================================
// The enabled gate: a disabled plugin does not run.
//
// Every test here drives `ArclainApp` itself rather than the session store
// underneath it, because the property under test is exactly that the
// refusal holds *whichever* facade entry point a frontend reaches for --
// a check that lives one layer down and is only exercised through a stub
// would prove nothing about the surface a renderer actually calls.
// ===========================================================================

#[test]
fn open_plugin_session_refuses_a_disabled_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .expect("disabling a known plugin must succeed");

    let error = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .expect_err("a disabled plugin must not open a session");

    assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);
    assert!(is_plugin_disabled_refusal(&error));
    // The gate and the read model answer from the same enabled flag, so
    // there is no state in which `plugins()` advertises a plugin the gate
    // then refuses to open, or the reverse.
    let summaries = runtime.block_on(app.plugins()).unwrap();
    assert!(
        !summaries
            .iter()
            .find(|summary| summary.id == "ui-demo")
            .expect("ui-demo is still reported while disabled")
            .enabled
    );
}

#[test]
fn open_plugin_session_for_archive_refuses_a_disabled_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    // The archive-browser info-panel path: it opens a slot for whatever
    // plugin a panel item names, so it is the entry point most likely to
    // reach a plugin the user has switched off.
    let error = runtime
        .block_on(app.open_plugin_session_for_archive(
            "ui-demo".to_string(),
            PluginExtensionPointDto::Panel,
            None,
        ))
        .expect_err("the explicit-origin open must be gated too");

    assert!(is_plugin_disabled_refusal(&error));
}

#[test]
fn the_disabled_refusal_is_distinguishable_from_an_unknown_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    let disabled = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .unwrap_err();
    let unknown = runtime
        .block_on(app.open_plugin_session(
            "does-not-exist".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap_err();

    // The two repairs are different -- draw nothing and keep the item vs.
    // drop a stale reference -- so a frontend must be able to tell them
    // apart without reading prose.
    assert_ne!(disabled.kind, unknown.kind);
    assert_eq!(unknown.kind, ApplicationErrorKind::NotFound);
    assert!(is_plugin_disabled_refusal(&disabled));
    assert!(!is_plugin_disabled_refusal(&unknown));
    // And the refusal names which plugin it is about, since the caller
    // may have asked on behalf of a slot it no longer holds the id for.
    assert_eq!(disabled.diagnostic.as_deref(), Some("plugin id: ui-demo"));
}

#[test]
fn a_disabled_plugin_stops_serving_the_document_of_a_session_that_was_already_open() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::Panel))
        .expect("opening while enabled must succeed");
    let before = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .expect("an enabled plugin's document is served");

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    // The panel is on screen and its plugin was just switched off: the
    // retained document is that plugin's own content, so it is withheld
    // rather than served one last time.
    let refused = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .expect_err("a disabled plugin's document must not be served");
    assert!(is_plugin_disabled_refusal(&refused));
    // Withheld, not discarded, and not a closed session either -- the
    // refusal is not the `NotFound` an unknown session id produces.
    assert_ne!(refused.kind, ApplicationErrorKind::NotFound);

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), true))
        .unwrap();

    let after = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .expect("re-enabling resumes the same session");
    assert_eq!(
        after, before,
        "a disable/enable round trip must not mint a new revision or re-fetch",
    );
}

#[test]
fn an_action_against_a_disabled_plugins_session_fails_the_operation_without_advancing_it() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .unwrap();
    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "demo_btn".to_string(),
            action: PluginActionDto::Activate,
        }))
        .expect("start_plugin_action accepts any well-formed request");

    // Refused where an unknown session is refused -- as the operation's
    // own terminal state, not a request-level `Err` -- so a frontend
    // watching the operation stream needs no second failure channel.
    match runtime.block_on(wait_for_terminal_state(&app, operation_id)) {
        OperationState::Failed { error } => assert!(
            is_plugin_disabled_refusal(&error),
            "expected the disabled refusal, got {error:?}",
        ),
        other => panic!("expected Failed for a disabled plugin's action, got {other:?}"),
    }

    // Nothing was published: no new revision, and the document that comes
    // back after re-enabling is the one the session already had.
    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), true))
        .unwrap();
    let document = runtime
        .block_on(app.plugin_ui_document(snapshot.session_id))
        .unwrap();
    assert_eq!(
        document, snapshot.document,
        "a refused dispatch must not advance the session",
    );
}

#[test]
fn close_plugin_session_still_succeeds_while_its_plugin_is_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();
    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string(), PluginExtensionPointDto::MainPage))
        .unwrap();
    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    // Teardown is the frontend's own bookkeeping, not something the
    // plugin does, so disabling must not turn correct cleanup into an
    // error path.
    runtime
        .block_on(app.close_plugin_session(snapshot.session_id))
        .expect("closing a disabled plugin's session must still succeed");

    let error = runtime
        .block_on(app.close_plugin_session(snapshot.session_id))
        .unwrap_err();
    assert_eq!(
        error.kind,
        ApplicationErrorKind::NotFound,
        "and the session really is gone afterwards",
    );
}

#[test]
fn disabling_one_plugin_leaves_every_other_plugins_session_working() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_plugins(&temp, &["ui-demo", "facade-test-fixture"]);
    let runtime = foreign_runtime();
    let fixture = runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::MainPage,
        ))
        .unwrap();

    runtime
        .block_on(app.set_plugin_enabled("ui-demo".to_string(), false))
        .unwrap();

    // Opening, reading and dispatching all still work for the plugin that
    // was not disabled -- the gate is per plugin, not a global switch.
    runtime
        .block_on(app.open_plugin_session(
            "facade-test-fixture".to_string(),
            PluginExtensionPointDto::Panel,
        ))
        .expect("an enabled plugin still opens sessions");
    runtime
        .block_on(app.plugin_ui_document(fixture.session_id))
        .expect("an enabled plugin still serves its document");
    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: fixture.session_id,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        }))
        .unwrap();
    let update = runtime.block_on(wait_for_plugin_ui_updated(&app, operation_id));
    assert_eq!(update.document.revision, 2);
    assert_eq!(
        update.intents.len(),
        3,
        "the fixture's own three host intents still arrive",
    );
}
