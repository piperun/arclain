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

mod common;

use std::sync::Arc;
use std::time::Duration;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::event::{OperationKind, OperationResult, OperationState};
use arclain_app::ids::{ArchiveSessionId, OperationId};
use arclain_app::plugins::{PluginActionDto, PluginHostIntentDto, PluginUiNodeKind};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{PluginSessions, PluginSlot, SlotView};
use arclain_ui::features::plugins::presentation::document_dispatch;
use arclain_ui::shared::image_assets::{ImageAssetState, ImageAssetStore, ImageOwner};
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
        archive_session: Some(ArchiveSessionId::from_raw(1)),
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
        archive_session: Some(ArchiveSessionId::from_raw(1)),
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
        archive_session: Some(ArchiveSessionId::from_raw(1)),
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

/// The lag path, driven through the **real** `reconcile_after_lag` rather
/// than by re-implementing its inner steps.
///
/// A `Lagged` broadcast receiver drops a plugin action's terminal event.
/// Without the plugin loop in `reconcile_after_lag`, the slot keeps
/// rendering a document that can never advance and the registry entry is
/// held for the life of the process. Deleting that loop must fail this
/// test -- which is exactly what the previous version of it did not do,
/// because it called `sessions.apply_update` by hand.
///
/// The dropped event is modelled the way
/// `operation_bridge_registration_race_test.rs` models it: nothing ever
/// subscribes, so the terminal event is gone by construction, and the
/// reconciler is invoked the same way the bridge's own loop invokes it on
/// `RecvError::Lagged`.
#[test]
fn reconcile_after_lag_recovers_a_plugin_action_whose_terminal_event_was_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let sessions = shared.plugin_sessions.clone();
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
            "precondition: the terminal event was never delivered, so it is still tracked"
        );

        arclain_ui::core::operation_bridge::reconcile_after_lag(
            &shared,
            &shared.operation_origins,
            &runtime,
            &app,
            1,
        )
        .await;

        assert!(
            sessions.tracked_ids().is_empty(),
            "reconciliation must drain the registry entry"
        );
        let SlotView::Ready(recovered) = sessions.view(&app, runtime.handle(), &slot) else {
            panic!("the slot must still hold a document");
        };
        assert!(
            recovered.revision > opened.revision,
            "reconciliation must apply the recovered document, not merely clear bookkeeping"
        );
    });
}

/// The image recovery loop through this frontend's own store, which is
/// the half `arclain_app`'s round-trip test cannot cover: that
/// `ImageAssetStore` routes a plugin key's *write* to the same namespace
/// its *read* resolves.
///
/// **Driven from inside a task on the store's own runtime**, which is the
/// production shape (`image_fetcher::trigger_image_fetch` stores from
/// exactly there) and the condition that distinguishes a working fix from
/// an inert one. The first attempt at this fix did the blocking write on
/// the calling thread; from a runtime worker that panics with "Cannot
/// start a runtime from within a runtime", so the recovery never ran in
/// production while a test calling it from a plain thread passed.
#[test]
fn the_image_store_writes_plugin_keys_where_it_reads_them_from_inside_a_runtime_task() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    // Built exactly as production does (`SharedState::new`), so this
    // exercises the real routing rather than a test-only source.
    let store = ImageAssetStore::with_plugin_images(None, app.clone(), runtime.clone());
    let key = "plugin-image:ui-demo:cover:RJ000002".to_string();
    let bytes = vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4];

    runtime.block_on({
        let store = store.clone();
        let app = app.clone();
        let key = key.clone();
        let bytes = bytes.clone();
        let runtime = runtime.clone();
        async move {
            // `spawn` + `await`, not a direct call: the future must be
            // polled by a runtime worker thread for this to reproduce the
            // production context at all.
            runtime
                .spawn(async move {
                    store
                        .store_fetched(
                            Some("ui-demo".to_string()),
                            key,
                            bytes,
                            Some("https://example.invalid/c.png".to_string()),
                            egui::Context::default(),
                        )
                        .await
                })
                .await
                .expect("the store task must not panic")
                .expect("a URL-fallback store must succeed for a plugin key");

            assert_eq!(
                app.read_plugin_image(key_for_read())
                    .await
                    .expect("the read that triggered the fetch must now find the bytes"),
                vec![0x89, 0x50, 0x4E, 0x47, 1, 2, 3, 4]
            );
        }
    });

    fn key_for_read() -> String {
        "plugin-image:ui-demo:cover:RJ000002".to_string()
    }
}

/// A plugin cannot write into another plugin's cache namespace by
/// authoring a key that names it.
///
/// The key encodes its own owner, so on its own it is a bearer token for
/// a namespace. `write_plugin_image` therefore takes the host's separate
/// statement of which plugin is acting and refuses a mismatch -- the
/// facade-side half of the guard, whose frontend-side half is the choke
/// points in `ImageAssetStore::request` and
/// `image_fetcher::trigger_image_fetch`.
#[test]
fn a_plugin_cannot_write_into_another_plugins_image_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let victim_key = "plugin-image:victim-plugin:secret".to_string();
    let poison = vec![0xDE, 0xAD, 0xBE, 0xEF];

    runtime.block_on(async {
        // Seed the victim's own entry, acting as the victim.
        app.write_plugin_image(
            "victim-plugin".to_string(),
            victim_key.clone(),
            vec![1, 2, 3, 4],
            None,
        )
        .await
        .expect("the owning plugin may write its own namespace");

        // Now the attacker, whose documents are rendered as "ui-demo",
        // tries to overwrite it by naming the victim in the key.
        let error = app
            .write_plugin_image("ui-demo".to_string(), victim_key.clone(), poison, None)
            .await
            .expect_err("a key naming another plugin must be refused");
        assert_eq!(error.kind, ApplicationErrorKind::PermissionDenied);

        assert_eq!(
            app.read_plugin_image(victim_key).await.unwrap(),
            vec![1, 2, 3, 4],
            "the victim's bytes must be untouched"
        );
    });
}

/// A structurally decodable but malformed owner cannot mint a cache
/// namespace of its own (each carries per-owner quota accounting).
#[test]
fn write_plugin_image_rejects_a_malformed_owner_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let bogus = "../../escape";

    let error = runtime
        .block_on(app.write_plugin_image(
            bogus.to_string(),
            format!("plugin-image:{bogus}:k"),
            vec![1, 2, 3],
            None,
        ))
        .expect_err("a malformed plugin id must be refused");

    assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
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
        archive_session: Some(ArchiveSessionId::from_raw(1)),
    };
    let departed = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(2),
        archive_session: Some(ArchiveSessionId::from_raw(2)),
    };
    let window_scoped = PluginSlot::PluginButton {
        plugin_id: "ui-demo".to_string(),
    };

    runtime.block_on(async {
        for slot in [&kept, &departed, &window_scoped] {
            view_until_resolved(&sessions, &app, runtime.handle(), slot).await;
        }
        assert_eq!(sessions.len(), 3);

        sessions.retain_hosts(
            &app,
            runtime.handle(),
            &[(TabId(1), Some(ArchiveSessionId::from_raw(1)))],
        );

        assert!(sessions.session_id(&kept).is_some());
        assert!(sessions.session_id(&departed).is_none());
        assert!(
            sessions.session_id(&window_scoped).is_some(),
            "a window-scoped slot belongs to no tab and must survive every tab close"
        );
    });
}

/// Pins the registration-race re-read -- `dispatch_action`'s own
/// `reconcile_started_action`, driven with the condition it exists for.
///
/// `start_plugin_action`'s worker can reach a terminal state and broadcast
/// before the caller resumes and records the operation id, so the bridge
/// drops that event as belonging to no slot. The re-read recovers it.
///
/// Exercised by tracking an operation that is *already* terminal, the same
/// "already finished before registration" shape
/// `operation_bridge_registration_race_test.rs` manufactures. Nothing
/// subscribes to the broadcast here, so the re-read is the only path by
/// which the outcome can arrive: remove it and the registry entry is never
/// drained and no toast appears.
///
/// Deliberately *not* driven through `dispatch_action` itself: that
/// function starts the operation, so whether it has finished microseconds
/// later is a genuine race -- a test asserting either outcome flakes
/// (measured at 1 failure in 3 runs before this was split out).
#[test]
fn the_started_action_re_read_applies_a_result_that_raced_its_registration() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&shared.plugin_sessions, &app, runtime.handle(), &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };

        let operation_id = shared
            .plugin_sessions
            .start_action(
                &app,
                &slot,
                "multi-action".to_string(),
                PluginActionDto::Activate,
            )
            .await
            .expect("starting an action must succeed");
        // Terminal *before* the re-read looks -- the race, forced open.
        wait_for_plugin_action_completion(&app, operation_id).await;
        assert!(
            shared.plugin_sessions.tracked_ids().contains(&operation_id),
            "precondition: no subscriber delivered the terminal event"
        );

        document_dispatch::reconcile_started_action(&shared, &app, operation_id).await;

        assert!(
            shared.plugin_sessions.tracked_ids().is_empty(),
            "the re-read must drain the registry entry"
        );
        let SlotView::Ready(applied) = shared.plugin_sessions.view(&app, runtime.handle(), &slot)
        else {
            panic!("the slot must still hold a document");
        };
        assert!(
            applied.revision > opened.revision,
            "the re-read must apply the recovered document, not merely clear bookkeeping"
        );
    });
}

/// N3's stranded-panel hazard: a `Panel` slot opened while no archive is
/// loaded must not become the answer forever.
///
/// The archive session is part of the slot key, so opening an archive
/// yields a *different* slot -- a fresh session, fetched against the
/// archive actually on screen. Without that, the archive-less (empty)
/// document would be cached under the same key and the panel would never
/// appear.
#[test]
fn a_panel_opened_without_an_archive_re_opens_once_one_is_active() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let archive_less = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
        archive_session: None,
    };
    let with_archive = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
        archive_session: Some(ArchiveSessionId::from_raw(4)),
    };

    runtime.block_on(async {
        view_until_resolved(&sessions, &app, runtime.handle(), &archive_less).await;
        let archive_less_session = sessions
            .session_id(&archive_less)
            .expect("the archive-less panel opened a session");

        view_until_resolved(&sessions, &app, runtime.handle(), &with_archive).await;
        let with_archive_session = sessions
            .session_id(&with_archive)
            .expect("activating an archive must open a fresh session, not reuse the stale one");
        assert_ne!(archive_less_session, with_archive_session);

        // The per-frame sweep then retires the slot whose archive is gone,
        // so the stale session does not outlive the archive it was
        // opened for.
        sessions.retain_hosts(
            &app,
            runtime.handle(),
            &[(TabId(1), Some(ArchiveSessionId::from_raw(4)))],
        );
        assert!(sessions.session_id(&archive_less).is_none());
        assert_eq!(
            sessions.session_id(&with_archive),
            Some(with_archive_session)
        );
    });
}

/// The layout editor's capability probe must not take over the browser's
/// rendering slot -- that is what silently un-pinned the plugin's metadata
/// writes and could strand the panel on an archive-less document.
#[test]
fn probing_an_extension_point_leaves_no_slot_behind() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();

    runtime.block_on(async {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let offers_panel = loop {
            if let Some(answer) = sessions.probe_extension_point(
                &app,
                runtime.handle(),
                "ui-demo",
                arclain_app::plugins::PluginExtensionPointDto::Panel,
            ) {
                break answer;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the probe never answered"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        assert!(offers_panel, "ui-demo does implement a Panel layout");
        assert!(
            sessions.is_empty(),
            "a capability probe must claim no rendering slot"
        );

        // And the plugin that does not implement one answers false rather
        // than leaving the editor to guess.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let offers_dialog = loop {
            if let Some(answer) = sessions.probe_extension_point(
                &app,
                runtime.handle(),
                "ui-demo",
                arclain_app::plugins::PluginExtensionPointDto::Dialog("nope".to_string()),
            ) {
                break answer;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the probe never answered"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(!offers_dialog);
        assert!(sessions.is_empty());
    });
}

// ============================================================================
// Cross-plugin key enforcement, at the choke points themselves.
//
// `image_key_is_addressable_by` is unit-tested as a predicate, but the
// predicate is not the protection -- the three call sites are. The read
// guard in particular is the *sole* barrier for cross-plugin reads:
// `read_plugin_image` takes only a key, so the facade structurally cannot
// check who is asking. These tests fail if any of the three guards is
// removed.
// ============================================================================

/// Read half: a surface belonging to one plugin cannot read a key naming
/// another plugin's cache namespace.
#[test]
fn the_image_store_refuses_a_read_for_another_plugins_key() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let store = ImageAssetStore::with_plugin_images(None, app.clone(), runtime.clone());
    let victim_key = "plugin-image:ui-demo:cover";
    let ctx = egui::Context::default();

    let refused = store.request(
        ImageOwner::plugin_panel("attacker", "properties", TabId(1)),
        victim_key,
        ctx.clone(),
    );
    assert!(
        matches!(refused, ImageAssetState::Failed(_)),
        "a key naming another plugin must be refused at the store, got {refused:?}"
    );

    // The owning plugin's own surface is unaffected -- the guard must
    // reject forgery, not plugin images in general.
    let allowed = store.request(
        ImageOwner::plugin_panel("ui-demo", "properties", TabId(1)),
        victim_key,
        ctx,
    );
    assert!(
        matches!(allowed, ImageAssetState::Loading),
        "the owning plugin must still be able to load its own key, got {allowed:?}"
    );
}

/// A lightbox opened by a plugin is ownership-checked like any other
/// plugin-scoped surface, now that its owner carries the acting plugin.
#[test]
fn the_image_store_refuses_a_lightbox_read_for_another_plugins_key() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let store = ImageAssetStore::with_plugin_images(None, app.clone(), runtime.clone());

    let refused = store.request(
        ImageOwner::Lightbox {
            tab: TabId(1),
            plugin_id: Some("attacker".to_string()),
        },
        "plugin-image:ui-demo:cover",
        egui::Context::default(),
    );

    assert!(
        matches!(refused, ImageAssetState::Failed(_)),
        "a plugin-opened lightbox must not read another plugin's key, got {refused:?}"
    );
}

/// Write half: a fetch for a key naming another plugin is refused before
/// any request is issued -- long before the bytes could be written into
/// that plugin's namespace.
#[test]
fn triggering_an_image_fetch_refuses_another_plugins_key() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let mut shared = common::create_test_shared_state();
    let runtime = shared.services.tokio_runtime.clone();
    shared.facade = Some(app.clone());
    // The production image source, so `can_store` answers as it does in a
    // real app rather than short-circuiting this test for the wrong reason.
    shared.image_assets = ImageAssetStore::with_plugin_images(None, app, runtime);
    let ctx = egui::Context::default();

    assert!(
        !arclain_ui::shared::image_fetcher::trigger_image_fetch(
            &shared,
            Some("attacker".to_string()),
            "https://example.invalid/c.png".to_string(),
            "plugin-image:ui-demo:cover".to_string(),
            ctx.clone(),
        ),
        "a fetch for another plugin's key must be refused before any request is issued"
    );

    assert!(
        arclain_ui::shared::image_fetcher::trigger_image_fetch(
            &shared,
            Some("ui-demo".to_string()),
            "https://example.invalid/c.png".to_string(),
            "plugin-image:ui-demo:cover".to_string(),
            ctx,
        ),
        "the owning plugin's own key must still dispatch"
    );
}

/// The lightbox ingress filter: a plugin's `OpenLightbox` cannot list an
/// image it does not own, so the index the user navigates matches what
/// they can actually see.
#[test]
fn the_lightbox_ingress_drops_images_the_acting_plugin_does_not_own() {
    use arclain_plugins::types::PluginAction;
    use arclain_ui::features::plugins::domain::state::PluginDialogState;
    use arclain_ui::features::plugins::presentation::controllers::plugin_controller::{
        process_action, ActionContext,
    };
    use arclain_ui::shared::dialogs::LightboxState;

    let lightbox = arclain_app::Signal::new(LightboxState::default());
    let mut dialog = PluginDialogState::default();
    let mut toaster = arclain_widgets::Toaster::new();
    let context = ActionContext {
        lightbox_signal: Some(&lightbox),
        page_display_name_signal: None,
        metadata_signal: None,
        shared_state: None,
        origin_tab: None,
    };

    process_action(
        PluginAction::OpenLightbox {
            images: vec![
                ("plugin-image:victim:secret".to_string(), None),
                ("own-unstamped-key".to_string(), None),
            ],
            start_index: 0,
            title: None,
        },
        "attacker",
        &mut dialog,
        &mut toaster,
        None,
        &context,
    );

    let state = lightbox.get();
    assert_eq!(
        state
            .images
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["own-unstamped-key"],
        "an image naming another plugin's namespace must not be listed at all"
    );
    assert_eq!(
        state.source_plugin.as_deref(),
        Some("attacker"),
        "the lightbox must record who opened it, so the store can check its reads too"
    );
}

/// The pin a panel's plugin session carries comes from the *slot key*, not
/// from the application's ambient active-session state.
///
/// Those could disagree: the frontend reports the active archive
/// asynchronously, so a panel opened in the same frame as an archive
/// activation could open before the report landed and pin the previously
/// active archive -- or none. That did not self-heal, because the
/// mis-pinned slot's key was already correct and nothing re-opened it.
///
/// Reproduced here by never reporting an active session at all (the
/// facade's own view stays `None` throughout, which is the losing side of
/// that race in its most extreme form) and asserting the session is still
/// pinned to the archive the slot names.
#[test]
fn a_panel_pins_the_archive_its_slot_names_not_the_ambient_active_session() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let archive = ArchiveSessionId::from_raw(9);
    let slot = PluginSlot::Panel {
        plugin_id: "ui-demo".to_string(),
        tab: TabId(1),
        archive_session: Some(archive),
    };

    runtime.block_on(async {
        // Deliberately never call `set_active_archive_session`: the
        // application still believes nothing is active.
        view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let session_id = sessions.session_id(&slot).expect("the panel opened");

        assert_eq!(
            app.plugin_session_archive_origin(session_id).await.unwrap(),
            Some(archive),
            "the pin must come from the slot key, not from ambient state that had not caught up"
        );
    });
}

/// A slot with no archive origin still pins nothing, so its fetches fall
/// back as documented -- the key is the source of truth in both
/// directions, not just when it names something.
#[test]
fn a_window_scoped_slot_pins_no_archive_even_when_one_is_active() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let sessions = PluginSessions::new();
    let slot = PluginSlot::PluginButton {
        plugin_id: "ui-demo".to_string(),
    };

    runtime.block_on(async {
        app.set_active_archive_session(Some(ArchiveSessionId::from_raw(3)))
            .await
            .expect("reporting an active session must succeed");

        view_until_resolved(&sessions, &app, runtime.handle(), &slot).await;
        let session_id = sessions.session_id(&slot).expect("the slot opened");

        assert_eq!(
            app.plugin_session_archive_origin(session_id).await.unwrap(),
            None,
            "a slot that names no archive must pin none, regardless of what is active"
        );
    });
}
