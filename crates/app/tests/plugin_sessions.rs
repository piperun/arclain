//! Integration tests for renderer-neutral plugin sessions: `plugins`,
//! `set_plugin_enabled`, `open_plugin_session`, `plugin_ui_document`,
//! `close_plugin_session`, `start_plugin_action`, and
//! `set_active_archive_session` -- all driven through `ArclainApp`'s
//! public facade against the real bundled `ui-demo` WASM fixture, the
//! same way a real frontend would.
//!
//! Every test is a plain (synchronous) `#[test]`, not `#[tokio::test]`,
//! for the same reason `crates/app/tests/archive_sessions.rs` uses that
//! pattern: `ArclainApp` owns its own Tokio runtime, and dropping it from
//! inside an async context panics. Each test builds `app` in sync code,
//! drives facade calls through one foreign `Runtime::block_on`, and lets
//! `app` drop only after `block_on` returns.

mod support;

use std::path::PathBuf;
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::{ArchiveSessionId, PluginSessionId};
use arclain_app::plugins::{PluginActionRequest, PluginUiDocument};
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

/// Copies the workspace's real, prebuilt `ui-demo` plugin fixture
/// (`plugins/ui-demo/{ui-demo.toml,ui-demo.wasm}`, built by `just
/// plugins`) into `plugins_dir/ui-demo/`, the folder-mode layout
/// `arclain_plugins::loader::PluginLoader::discover_plugins` expects.
/// Exercising a real, running plugin instance (rather than a hand-built
/// `arclain_plugins::types::PluginLayout`) proves the whole path end to
/// end: WASM `get-ui-layout`/`on-ui-event` calls, normalization, and the
/// facade session/action wiring together.
fn install_ui_demo_fixture(plugins_dir: &std::path::Path) {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join("ui-demo");
    let dest_dir = plugins_dir.join("ui-demo");
    std::fs::create_dir_all(&dest_dir).expect("create plugin fixture directory");
    std::fs::copy(
        fixture_dir.join("ui-demo.wasm"),
        dest_dir.join("ui-demo.wasm"),
    )
    .expect("copy ui-demo.wasm fixture");
    std::fs::copy(
        fixture_dir.join("ui-demo.toml"),
        dest_dir.join("ui-demo.toml"),
    )
    .expect("copy ui-demo.toml fixture");
}

/// Bootstraps an `ArclainApp` against an isolated temp profile with the
/// real `ui-demo` plugin installed and a working (dummy-path) 7-Zip --
/// see `archive_sessions.rs::bootstrap_app`'s identical rationale for the
/// 7-Zip seed.
fn bootstrap_app_with_ui_demo(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    install_ui_demo_fixture(&paths.plugins_dir);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
    })
    .expect("bootstrap with the ui-demo fixture must succeed")
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

#[test]
fn open_plugin_session_normalizes_the_main_page_layout_and_documents_match_the_immediate_query() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app_with_ui_demo(&temp);
    let runtime = foreign_runtime();

    let snapshot = runtime
        .block_on(app.open_plugin_session("ui-demo".to_string()))
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
        .block_on(app.open_plugin_session("does-not-exist".to_string()))
        .unwrap_err();

    assert_eq!(error.kind, ApplicationErrorKind::NotFound);
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
        .block_on(app.open_plugin_session("ui-demo".to_string()))
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
        .block_on(app.open_plugin_session("ui-demo".to_string()))
        .unwrap();
    let mut events = app.subscribe_operations();

    let operation_id = runtime
        .block_on(app.start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: "demo_btn".to_string(),
            action: arclain_plugins::ui_model::PluginActionDto::Activate,
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
            action: arclain_plugins::ui_model::PluginActionDto::Activate,
        }))
        .expect("start_plugin_action itself accepts any well-formed request");

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let final_state = loop {
        let snapshot = runtime.block_on(app.operation(operation_id)).unwrap();
        match snapshot.state {
            OperationState::Failed { .. } | OperationState::Completed { .. } => {
                break snapshot.state
            }
            _ if std::time::Instant::now() < deadline => {
                runtime.block_on(tokio::time::sleep(Duration::from_millis(10)));
            }
            _ => panic!("operation did not reach a terminal state within the test deadline"),
        }
    };

    match final_state {
        OperationState::Failed { error } => assert_eq!(error.kind, ApplicationErrorKind::NotFound),
        other => panic!("expected Failed(NotFound) for an unknown session, got {other:?}"),
    }
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
        extension_point: arclain_plugins::ui_model::PluginExtensionPointDto::MainPage,
        revision: 3,
        root,
    };
    let result = OperationResult::PluginUiUpdated {
        update: arclain_app::plugins::PluginUiUpdate {
            document,
            intents: vec![arclain_plugins::ui_model::PluginHostIntentDto::ShowToast {
                message: "done".to_string(),
                level: arclain_plugins::ui_model::PluginToastLevelDto::Success,
            }],
        },
    };

    let json = serde_json::to_string(&result).unwrap();
    let restored: OperationResult = serde_json::from_str(&json).unwrap();

    assert_eq!(restored, result);
    assert!(json.contains("\"type\":\"plugin_ui_updated\""));
}
