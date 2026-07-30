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
// Every plugin-UI DTO type is reached through `arclain_app::plugins`, not
// `arclain_plugins` directly -- proving the facade re-exports the full
// transitive surface `PluginUiDocument`/`PluginActionRequest` expose (see
// that module's own doc comment). `PluginLayout`/`PluginUiElement`/
// `normalize_layout` are the one deliberate exception used later in this
// file: they are pre-normalization, WIT-facing types nothing outside
// `arclain_plugins` ever receives from the real facade, only useful here
// to hand-build a sample document for one serde round-trip test.
use arclain_app::plugins::{
    PluginActionDto, PluginActionRequest, PluginExtensionPointDto, PluginHostIntentDto,
    PluginToastLevelDto, PluginUiDocument,
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

/// Copies a workspace plugin fixture (`plugins/{name}/{name}.toml`,
/// `{name}.wasm`, built by `just plugins`) into `plugins_dir/{name}/`,
/// the folder-mode layout `arclain_plugins::loader::PluginLoader::
/// discover_plugins` expects. Exercising a real, running plugin instance
/// (rather than a hand-built `arclain_plugins::types::PluginLayout`)
/// proves the whole path end to end: WASM `get-ui-layout`/`on-ui-event`
/// calls, normalization, and the facade session/action wiring together.
fn install_plugin_fixture(plugins_dir: &std::path::Path, name: &str) {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name);
    let dest_dir = plugins_dir.join(name);
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
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(temp));
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    install_plugin_fixture(&paths.plugins_dir, plugin_name);
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
