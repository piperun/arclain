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
use arclain_ui::features::plugins::presentation::rendering::DocumentEvent;
use arclain_ui::shared::image_assets::{ImageAssetState, ImageAssetStore, ImageOwner};
use eframe::egui;

fn bootstrap_with_plugin(temp: &tempfile::TempDir, plugin: &str) -> ArclainApp {
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    std::fs::create_dir_all(&paths.plugins_dir).expect("create plugins dir");
    common::install_plugin_fixture(&paths.plugins_dir, plugin);
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
                opened.session_id,
                opened.revision,
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

/// A delayed dispatch must retain the revision from the document that
/// rendered the event. If `start_action` re-reads the open slot instead,
/// this revision-1 event is relabelled revision 2 and incorrectly reaches
/// the guest instead of failing with the app facade's stale-action conflict.
#[test]
fn a_delayed_document_event_keeps_its_rendered_revision() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.handle().clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&shared.plugin_sessions, &app, &runtime, &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };
        assert_eq!(opened.revision, 1);
        let stale_event = DocumentEvent::Interact {
            expected_session_id: opened.session_id,
            expected_revision: opened.revision,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        };

        let advancing_operation = shared
            .plugin_sessions
            .start_action(
                &app,
                &slot,
                opened.session_id,
                opened.revision,
                "multi-action".to_string(),
                PluginActionDto::Activate,
            )
            .await
            .expect("the revision-1 action must start");
        let OperationState::Completed {
            result: OperationResult::PluginUiUpdated { update },
        } = wait_for_plugin_action_completion(&app, advancing_operation).await
        else {
            panic!("the revision-1 action must advance the document");
        };
        let applied = shared
            .plugin_sessions
            .apply_update(advancing_operation, update)
            .expect("the registry must apply the advancing document");
        assert_eq!(applied.document.revision, 2);

        let mut operations = app.subscribe_operations();
        document_dispatch::apply_document_events(&shared, &slot, TabId(1), vec![stale_event]);
        let stale_operation = loop {
            let event = tokio::time::timeout(Duration::from_secs(30), operations.recv())
                .await
                .expect("the delayed document event must start an operation")
                .expect("the operation subscription must stay live");
            if event.kind == OperationKind::PluginAction {
                break event.operation_id;
            }
        };
        match wait_for_plugin_action_completion(&app, stale_operation).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::Conflict)
            }
            other => panic!("the revision-1 event must remain stale, got {other:?}"),
        }
    });
}

/// A delayed event must target the exact session whose document rendered it.
/// If dispatch substitutes the slot's replacement session id, this old
/// session-A/revision-1 event is admitted against session B at revision 1.
#[test]
fn a_delayed_document_event_keeps_its_rendered_session_identity() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "facade-test-fixture");
    let mut shared = common::create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.handle().clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&shared.plugin_sessions, &app, &runtime, &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };
        assert_eq!(opened.revision, 1);
        let stale_event = DocumentEvent::Interact {
            expected_session_id: opened.session_id,
            expected_revision: opened.revision,
            node_id: "multi-action".to_string(),
            action: PluginActionDto::Activate,
        };

        shared.plugin_sessions.close(&app, &runtime, &slot);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match app.plugin_ui_document(opened.session_id).await {
                Err(error) if error.kind == ApplicationErrorKind::NotFound => break,
                Ok(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the original session never closed"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("closing the original session failed: {error:?}"),
            }
        }

        let SlotView::Ready(replacement) =
            view_until_resolved(&shared.plugin_sessions, &app, &runtime, &slot).await
        else {
            panic!("the replacement slot must resolve to a document");
        };
        assert_ne!(replacement.session_id, opened.session_id);
        assert_eq!(replacement.revision, 1);

        let mut operations = app.subscribe_operations();
        document_dispatch::apply_document_events(&shared, &slot, TabId(1), vec![stale_event]);
        let stale_operation = loop {
            let event = tokio::time::timeout(Duration::from_secs(30), operations.recv())
                .await
                .expect("the delayed document event must start an operation")
                .expect("the operation subscription must stay live");
            if event.kind == OperationKind::PluginAction {
                break event.operation_id;
            }
        };
        match wait_for_plugin_action_completion(&app, stale_operation).await {
            OperationState::Failed { error } => {
                assert_eq!(error.kind, ApplicationErrorKind::NotFound)
            }
            other => panic!("the old-session event must not reach its replacement, got {other:?}"),
        }
        let retained = app
            .plugin_ui_document(replacement.session_id)
            .await
            .expect("the replacement session must remain open");
        assert_eq!(retained.revision, 1);
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
        let SlotView::Ready(opened) =
            view_until_resolved(&sessions, &app, runtime.handle(), &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };
        let session_id = sessions.session_id(&slot).expect("session opened");

        let operation_id = sessions
            .start_action(
                &app,
                &slot,
                opened.session_id,
                opened.revision,
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
    let runtime = shared.services.tokio_runtime.handle().clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let sessions = shared.plugin_sessions.clone();
        let SlotView::Ready(opened) = view_until_resolved(&sessions, &app, &runtime, &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };

        let operation_id = sessions
            .start_action(
                &app,
                &slot,
                opened.session_id,
                opened.revision,
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
        let SlotView::Ready(recovered) = sessions.view(&app, &runtime, &slot) else {
            panic!("the slot must still hold a document");
        };
        assert!(
            recovered.revision > opened.revision,
            "reconciliation must apply the recovered document, not merely clear bookkeeping"
        );
    });
}

/// A single-URL HTTP stub serving one PNG, so a URL-fallback fetch has
/// something real to fetch.
///
/// The application's *plugin* request path refuses loopback addresses
/// (special-address validation on the resolved IPs), so only the host
/// namespace can be exercised against a local stub -- which is exactly the
/// namespace `arclain_app`'s own tests cannot reach through this
/// frontend's store.
struct ImageStub {
    address: std::net::SocketAddr,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl ImageStub {
    fn start(body: Vec<u8>, content_type: &'static str) -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the image stub");
        let address = listener.local_addr().expect("read the stub address");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || {
            while let Ok((mut socket, _)) = listener.accept() {
                if stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request = Vec::new();
                let mut chunk = [0_u8; 512];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                // Answer only a complete request head. Loopback ephemeral
                // ports are shared with every test binary running in
                // parallel, so a connect that sends nothing is somebody
                // else's port probe, not a fetch.
                if !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    continue;
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                );
                let _ = socket.write_all(header.as_bytes());
                let _ = socket.write_all(&body);
                let _ = socket.flush();
            }
        });
        Self { address, stop }
    }

    fn url(&self) -> String {
        format!("http://{}/cover.png", self.address)
    }
}

impl Drop for ImageStub {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(self.address);
    }
}

/// A decodable PNG large enough to clear the fetch path's "real images are
/// >1KB" floor. The size assertion is not decoration: a smaller payload
/// would make the fetch fail for a reason this test is not about.
///
/// `tint` keeps each test's bytes distinct. Cache blobs are
/// content-addressed in a store every bootstrapped application shares (the
/// cache root ignores `paths_override` — see this task's report), so two
/// tests with identical bytes would share one physical blob and could
/// delete it out from under each other.
fn fetchable_png(tint: u8) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(64, 64, |x, y| {
        image::Rgba([
            (x * 7 + y * 13) as u8,
            (x * 29 + 11) as u8,
            (y * 31 + 5) as u8,
            tint,
        ])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode PNG");
    let bytes = bytes.into_inner();
    assert!(
        bytes.len() > 1000,
        "the fixture PNG must clear the fetch path's minimum size"
    );
    bytes
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the image worker"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The URL-fallback loop through this frontend's own store: that
/// `ImageAssetStore` fetches a key into the same namespace its *read*
/// resolves, so the read that asked for the fetch can find the bytes
/// afterwards.
///
/// **Driven from inside a task on the store's own runtime**, which is the
/// production shape (`image_fetcher::trigger_image_fetch` fetches from
/// exactly there) and the condition that distinguishes a working fix from
/// an inert one. An earlier version of this path blocked on the runtime
/// from the calling thread; from a runtime worker that panics with "Cannot
/// start a runtime from within a runtime", so the recovery never ran in
/// production while a test calling it from a plain thread passed.
#[test]
fn the_image_store_fetches_host_keys_where_it_reads_them_from_inside_a_runtime_task() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    // Built exactly as production does (`SharedState::new`), so this
    // exercises the real routing rather than a test-only source.
    let store = ImageAssetStore::new(app.clone(), runtime.handle().clone());
    let png = fetchable_png(255);
    let stub = ImageStub::start(png.clone(), "image/png");
    let key = "dlsite:image:host-fetch-round-trip".to_string();

    runtime.block_on({
        let store = store.clone();
        let app = app.clone();
        let key = key.clone();
        let url = stub.url();
        let runtime = runtime.clone();
        async move {
            // `spawn` + `await`, not a direct call: the future must be
            // polled by a runtime worker thread for this to reproduce the
            // production context at all.
            runtime
                .spawn(async move {
                    store
                        .fetch_into_cache(None, key, url, egui::Context::default())
                        .await
                })
                .await
                .expect("the store task must not panic")
                .expect("a URL-fallback fetch must succeed for a host key");

            assert_eq!(
                app.read_host_image(key_for_read())
                    .await
                    .expect("the read that triggered the fetch must now find the bytes"),
                png
            );
        }
    });

    // ...and the store's own read resolves the same namespace, which is
    // what makes the asset render rather than re-fetch every 30 s.
    let owner = ImageOwner::plugin_panel("ui-demo", "properties", TabId(1));
    assert!(matches!(
        store.request(owner, &key, egui::Context::default()),
        ImageAssetState::Loading
    ));
    wait_until(|| store.is_decoded(&key));

    fn key_for_read() -> String {
        "dlsite:image:host-fetch-round-trip".to_string()
    }
}

/// The plugin half of the same routing question, without a network hop:
/// a key already present in the *plugin's* namespace resolves through the
/// store's fetch path untouched.
///
/// This is what proves the fetch takes the plugin route rather than the
/// host one. A host-routed fetch would be refused outright (a plugin-scoped
/// key is `PermissionDenied` on every host method), so "succeeds, from
/// cache, and the store then decodes it" can only happen if the fetch and
/// the read agree on the namespace.
#[test]
fn the_image_store_resolves_plugin_keys_through_the_plugins_own_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let runtime = runtime();
    let store = ImageAssetStore::new(app.clone(), runtime.handle().clone());
    let key = "plugin-image:ui-demo:cover:RJ000002".to_string();
    let png = fetchable_png(254);

    runtime.block_on({
        let store = store.clone();
        let app = app.clone();
        let key = key.clone();
        let png = png.clone();
        let runtime = runtime.clone();
        async move {
            app.write_plugin_image("ui-demo".to_string(), key.clone(), png, None)
                .await
                .expect("the owning plugin's namespace accepts its own key");

            runtime
                .spawn(async move {
                    store
                        .fetch_into_cache(
                            Some("ui-demo".to_string()),
                            key,
                            "https://example.invalid/c.png".to_string(),
                            egui::Context::default(),
                        )
                        .await
                })
                .await
                .expect("the store task must not panic")
                .expect("a warm plugin key must resolve without reaching the network");
        }
    });

    let owner = ImageOwner::plugin_panel("ui-demo", "properties", TabId(1));
    assert!(matches!(
        store.request(owner, &key, egui::Context::default()),
        ImageAssetState::Loading
    ));
    wait_until(|| store.is_decoded(&key));
}

/// The renderer's 30 s retry must obey the application's own verdict --
/// both ways.
///
/// The application already classifies a non-image body (like an oversized
/// one) as `Recoverability::Fatal`, but that classification used to be
/// flattened into a bare message at this frontend's error boundary, one
/// call before the only code that could act on it. So an asset the
/// application would never accept was re-fetched every 30 s forever. This
/// drives the whole chain -- facade verdict, `ImageFetchError`, the
/// asset's recorded state, and `trigger_image_fetch`'s refusal -- and
/// pins the other direction too, because "stop after any failure" would
/// break every transient retry.
#[test]
fn a_permanently_refused_image_stops_refetching_while_a_transient_one_does_not() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_with_plugin(&temp, "ui-demo");
    let mut shared = common::create_test_shared_state();
    let runtime = shared.services.tokio_runtime.handle().clone();
    shared.facade = Some(app.clone());
    shared.image_assets = ImageAssetStore::new(app, runtime.clone());
    let ctx = egui::Context::default();

    // A body the application refuses permanently: right size, wrong type.
    let refused = ImageStub::start(fetchable_png(253), "text/html");
    // An address nothing is listening on: a transport failure, which is
    // exactly the kind that *should* keep retrying.
    let unreachable = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{address}/cover.png")
    };

    for (key, url, may_help_after, note) in [
        (
            "dlsite:image:permanently-refused",
            refused.url(),
            false,
            "a fatally refused asset must stop asking",
        ),
        (
            "dlsite:image:transiently-failed",
            unreachable,
            true,
            "a transport failure must still retry",
        ),
    ] {
        // A fetch is only attempted once the read has failed, so put the
        // asset in the state production would put it in.
        shared.image_assets.request(
            ImageOwner::plugin_panel("ui-demo", "properties", TabId(1)),
            key,
            ctx.clone(),
        );
        wait_until(|| {
            matches!(
                shared.image_assets.state(key),
                Some(ImageAssetState::Failed(_))
            )
        });
        assert!(
            shared.image_assets.fetch_may_help(key),
            "the first attempt must always be allowed: {note}"
        );

        runtime
            .block_on(
                shared
                    .image_assets
                    .fetch_into_cache(None, key.to_string(), url, ctx.clone()),
            )
            .expect_err("both fixtures fail");

        assert_eq!(
            shared.image_assets.fetch_may_help(key),
            may_help_after,
            "{note}"
        );
        assert_eq!(
            arclain_ui::shared::image_fetcher::trigger_image_fetch(
                &shared,
                None,
                "https://example.invalid/c.png".to_string(),
                key.to_string(),
                ctx.clone(),
            ),
            may_help_after,
            "the fetch choke point must agree with the recorded verdict: {note}"
        );
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
    let runtime = shared.services.tokio_runtime.handle().clone();
    let slot = PluginSlot::MainPage {
        plugin_id: "facade-test-fixture".to_string(),
    };

    runtime.block_on(async {
        let SlotView::Ready(opened) =
            view_until_resolved(&shared.plugin_sessions, &app, &runtime, &slot).await
        else {
            panic!("the main-page slot must resolve to a document");
        };

        let operation_id = shared
            .plugin_sessions
            .start_action(
                &app,
                &slot,
                opened.session_id,
                opened.revision,
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
        let SlotView::Ready(applied) = shared.plugin_sessions.view(&app, &runtime, &slot) else {
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
    let store = ImageAssetStore::new(app.clone(), runtime.handle().clone());
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
    let store = ImageAssetStore::new(app.clone(), runtime.handle().clone());

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
    let runtime = shared.services.tokio_runtime.handle().clone();
    shared.facade = Some(app.clone());
    // The production image source, so `can_store` answers as it does in a
    // real app rather than short-circuiting this test for the wrong reason.
    shared.image_assets = ImageAssetStore::new(app, runtime);
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
