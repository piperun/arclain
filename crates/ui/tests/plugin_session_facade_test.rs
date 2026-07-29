//! End-to-end coverage for egui's facade-backed plugin UI path, against
//! real, running WASM plugin fixtures.
//!
//! `facade_sessions`'s own unit tests drive the routing rules with
//! hand-built documents; the render tests drive real clicks against
//! hand-built documents. This file closes the remaining gap: a real
//! `ArclainApp`, a real plugin instance, and the registry driving the
//! actual facade calls -- `open_plugin_session` producing a real
//! normalized document, `start_plugin_action` round-tripping through a
//! real `on-ui-event`, and `close_plugin_session` actually releasing the
//! session.
//!
//! Uses `ui-demo` (which implements `Panel`) and `facade-test-fixture`
//! (whose `on-ui-event` returns real actions, unlike `ui-demo`'s, which
//! always returns an empty list). Deliberately does *not* use
//! `dlsite-metadata`: a debug-profile build of that fixture exceeds the
//! wasmtime fuel budget on `OnArchiveOpen`, which is tracked separately
//! and unrelated to this path.
//!
//! Every test is a plain `#[test]` driving one foreign `block_on`, for
//! the same reason `crates/app/tests/plugin_sessions.rs` gives: the
//! application owns its own runtime, and dropping it from inside an async
//! context panics.

use std::sync::Arc;
use std::time::Duration;

use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::plugins::{PluginActionDto, PluginHostIntentDto, PluginUiNodeKind};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{PluginSessions, PluginSlot, SlotView};
use arclain_ui::shared::image_assets::ImageAssetStore;
use eframe::egui;

/// Copies a workspace plugin fixture into the folder layout the plugin
/// loader expects -- mirrors `crates/app/tests/plugin_sessions.rs`'s
/// helper of the same name (each test binary is its own crate).
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

fn bootstrap_with_plugin(temp: &tempfile::TempDir, plugin: &str) -> ArclainApp {
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    install_plugin_fixture(&paths.plugins_dir, plugin);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap with a plugin fixture must succeed")
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build the test runtime"),
    )
}

/// Polls `view` until the slot leaves `Opening`, the way a render loop
/// does -- the first call spawns the open, later calls observe it.
async fn view_until_resolved(
    sessions: &PluginSessions,
    app: &ArclainApp,
    handle: &tokio::runtime::Handle,
    slot: &PluginSlot,
) -> SlotView {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match sessions.view(app, handle, slot) {
            SlotView::Opening => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the plugin session never finished opening"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            resolved => return resolved,
        }
    }
}

async fn wait_for_plugin_action_completion(
    app: &ArclainApp,
    operation_id: OperationId,
) -> OperationState {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = app.operation(operation_id).await.expect("operation exists");
        assert_eq!(snapshot.kind, OperationKind::PluginAction);
        match snapshot.state {
            OperationState::Completed { .. }
            | OperationState::Failed { .. }
            | OperationState::Cancelled => return snapshot.state,
            _ => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the plugin action never reached a terminal state"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

fn label_texts(node: &arclain_app::plugins::PluginUiNodeDto) -> Vec<String> {
    let mut texts = Vec::new();
    collect_labels(node, &mut texts);
    texts
}

fn collect_labels(node: &arclain_app::plugins::PluginUiNodeDto, out: &mut Vec<String>) {
    match &node.kind {
        PluginUiNodeKind::Label { text, .. } => out.push(text.clone()),
        PluginUiNodeKind::Single { children }
        | PluginUiNodeKind::ListContainer { children, .. }
        | PluginUiNodeKind::Group { children, .. } => {
            for child in children {
                collect_labels(child, out);
            }
        }
        PluginUiNodeKind::Split {
            sidebar, content, ..
        } => {
            for child in sidebar.iter().chain(content) {
                collect_labels(child, out);
            }
        }
        _ => {}
    }
}

/// The panel path the archive browser now renders through: declaring the
/// slot opens a real session and produces the plugin's real `Panel`
/// document.
#[test]
fn declaring_a_panel_slot_opens_a_real_session_and_yields_the_plugins_panel_document() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
    };

    runtime.block_on(async {
        let view = view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let SlotView::Ready(document) = view else {
            panic!("the panel slot must resolve to a document, got {view:?}");
        };
        assert_eq!(document.plugin_id, "ui-demo");
        assert_eq!(document.region_id, "panel");
        assert_eq!(
            label_texts(&document.root),
            vec!["Plugin Info".to_string(), "Status: Active".to_string()],
            "the document must carry ui-demo's real Panel layout"
        );
        assert!(sessions.session_id(&slot).is_some());
    });
}

/// Re-declaring the same slot every frame must not re-enter the guest --
/// the property the session model has structurally and the old
/// fetch-by-key cache needed explicit coalescing for.
#[test]
fn re_declaring_a_slot_reuses_its_session_rather_than_opening_another() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
    };

    runtime.block_on(async {
        view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let first = sessions.session_id(&slot).expect("session opened");
        for _ in 0..5 {
            let _ = sessions.view(&app, runtime.handle(), &slot);
        }
        assert_eq!(sessions.session_id(&slot), Some(first));
        assert_eq!(sessions.len(), 1);
    });
}

/// The full round trip: dispatch an interaction against a real plugin,
/// let the facade run it, and route the resulting operation through the
/// registry exactly as `operation_bridge` does.
#[test]
fn an_action_round_trips_through_the_facade_and_advances_the_slots_document() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&sessions, &app, runtime.handle(), &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };
        assert_eq!(opened.revision, 1);

        let operation_id = sessions
            .start_action(
                &app,
                &slot,
                "multi-action".to_string(),
                PluginActionDto::Activate,
            )
            .await
            .expect("starting an action against an open slot must succeed");

        // Re-read the operation's own snapshot rather than racing the
        // broadcast: a fast action can complete before `start_action`
        // returns, which is exactly the window
        // `document_dispatch::dispatch_action` reconciles in production.
        let OperationState::Completed {
            result: OperationResult::PluginUiUpdated { update },
        } = wait_for_plugin_action_completion(&app, operation_id).await
        else {
            panic!("the action must complete with an updated document");
        };

        let applied = sessions
            .apply_update(operation_id, update)
            .expect("the registry must accept its own operation's update");
        assert_eq!(applied.slot, slot);
        assert!(
            applied.document.revision > opened.revision,
            "the applied document must advance the slot's revision"
        );
        assert_eq!(
            applied.intents,
            vec![
                PluginHostIntentDto::ShowToast {
                    message: "first".to_string(),
                    level: arclain_app::plugins::PluginToastLevelDto::Info,
                },
                PluginHostIntentDto::CopyToClipboard {
                    text: "second".to_string(),
                },
                PluginHostIntentDto::SetPageDisplayName {
                    name: "third".to_string(),
                },
            ],
            "intents must reach the renderer in the order the plugin emitted them"
        );

        // The same update cannot be routed a second time.
        assert!(sessions.fail(operation_id).is_none());
    });
}

/// A plugin trap must not take the host down, must surface as a failed
/// operation, and must leave the slot's last good document intact.
#[test]
fn a_trapping_action_fails_the_operation_without_discarding_the_slots_document() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let session_id = sessions.session_id(&slot).expect("session opened");

        let operation_id = sessions
            .start_action(
                &app,
                &slot,
                "trigger-trap".to_string(),
                PluginActionDto::Activate,
            )
            .await
            .expect("starting an action against an open slot must succeed");
        assert!(matches!(
            wait_for_plugin_action_completion(&app, operation_id).await,
            OperationState::Failed { .. }
        ));

        assert_eq!(sessions.fail(operation_id), Some(slot.clone()));
        assert_eq!(
            sessions.session_id(&slot),
            Some(session_id),
            "a failed action keeps the slot's session and last good document"
        );
    });
}

/// Closing a slot releases its facade session rather than leaking it in
/// the application's session store.
#[test]
fn closing_a_slot_releases_its_facade_session() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
    };

    runtime.block_on(async {
        view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let session_id = sessions.session_id(&slot).expect("session opened");
        assert!(app.plugin_ui_document(session_id).await.is_ok());

        sessions.close(&app, runtime.handle(), &slot);
        assert!(sessions.is_empty());

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if app.plugin_ui_document(session_id).await.is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "closing the slot must close its facade session"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
}

/// The lag path: a `Lagged` broadcast receiver drops a plugin action's
/// terminal event, and reconciliation must still deliver the document and
/// drain the registry. Without it the slot renders a document that can
/// never advance and its operation entry is held for the process's life.
///
/// The dropped event is simulated by never feeding the broadcast at all
/// and calling the reconciler directly -- the same way
/// `operation_bridge_registration_race_test.rs` drives `reconcile_after_lag`
/// rather than manufacturing a real channel overflow.
#[test]
fn a_lagged_terminal_event_is_recovered_by_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&sessions, &app, runtime.handle(), &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };

        let operation_id = sessions
            .start_action(
                &app,
                &slot,
                "multi-action".to_string(),
                PluginActionDto::Activate,
            )
            .await
            .expect("starting an action must succeed");
        wait_for_plugin_action_completion(&app, operation_id).await;

        assert!(
            sessions.tracked_ids().contains(&operation_id),
            "the operation must still be tracked -- its terminal event was 'dropped'"
        );

        // What the lag handler does for each tracked id: re-read the
        // operation's own snapshot and route it as if it had arrived.
        let snapshot = app.operation(operation_id).await.unwrap();
        let OperationState::Completed {
            result: OperationResult::PluginUiUpdated { update },
        } = snapshot.state
        else {
            panic!("the action must have completed with a document");
        };
        let applied = sessions
            .apply_update(operation_id, update)
            .expect("reconciliation must apply the recovered update");

        assert!(applied.document.revision > opened.revision);
        assert!(
            sessions.tracked_ids().is_empty(),
            "reconciliation must drain the registry, not just apply the document"
        );
    });
}

/// The image recovery loop through this frontend's own store, which is
/// the half `arclain_app`'s round-trip test cannot cover: that
/// `ImageAssetStore` routes a plugin key's *write* to the same namespace
/// its *read* resolves.
///
/// Before both halves shared one `ImageBytes` implementation, the store
/// read through `read_plugin_image` (plugin namespace, decoded key) while
/// the fetch site wrote straight to the content cache (host namespace,
/// encoded key) -- structurally distinct, so a URL-fallback recovery could
/// never satisfy the read that triggered it.
#[test]
fn the_image_store_writes_plugin_keys_where_it_reads_them() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    // Built exactly as production does (`SharedState::new`), so this
    // exercises the real routing rather than a test-only source.
    let store = ImageAssetStore::with_plugin_images(None, app.clone(), runtime.clone());
    let key = "plugin-image:ui-demo:cover:RJ000002";
    let bytes = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4];

    store
        .store_fetched(
            key,
            &bytes,
            Some("https://example.invalid/c.png"),
            egui::Context::default(),
        )
        .expect("a URL-fallback store must succeed for a plugin key");

    assert_eq!(
        runtime
            .block_on(app.read_plugin_image(key.to_string()))
            .expect("the read that triggered the fetch must now find the bytes"),
        bytes
    );
}

/// Every tab-scoped slot of a closing tab is swept, and window-scoped
/// slots survive -- the invariant
/// `app_lifecycle::sweep_closed_tab_plugin_sessions` relies on.
#[test]
fn retaining_open_tabs_closes_only_the_departed_tabs_slots() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let kept = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
    };
    let departed = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(2),
    };
    let window_scoped = PluginSlot::PluginButton {
        plugin_id: "ui-demo".to_string(),
    };

    runtime.block_on(async {
        for slot in [&kept, &departed, &window_scoped] {
            view_until_resolved(&sessions, &app, runtime.handle(), slot).await;
        }
        assert_eq!(sessions.len(), 3);

        sessions.retain_tabs(&app, runtime.handle(), &[TabId(1)]);

        assert!(sessions.session_id(&kept).is_some());
        assert!(sessions.session_id(&departed).is_none());
        assert!(
            sessions.session_id(&window_scoped).is_some(),
            "a window-scoped slot belongs to no tab and must survive every tab close"
        );
    });
}
