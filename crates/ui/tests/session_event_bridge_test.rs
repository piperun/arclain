//! Parity coverage for the plugin-bridge swap: `ArclainApp::
//! active_tab_bridge` is now installed on `PluginManager` in production,
//! replacing the old bridge that wrote directly into egui's own tab
//! signals. Every case a plugin's metadata write can take must still
//! reach the correct tab -- just through a different mechanism (a
//! `SessionEvent` the session-event consumer in `operation_bridge`
//! reacts to, rather than a direct signal write inside the trait call
//! itself).
//!
//! Drives the installed bridge's own trait methods directly (exactly
//! what a plugin host function does), then drives the consumer
//! functions directly too (mirroring `operation_bridge_registration_race_
//! test.rs`'s own established pattern of calling `register_operation`/
//! `reconcile_after_lag` directly rather than exercising the live spawned
//! loop) -- proving the logic without needing a real WASM plugin or a
//! live broadcast subscription.

mod common;
use common::create_test_shared_state;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arclain_app::archive::OpenArchiveRequest;
use arclain_app::event::SessionEvent;

/// A backend whose `list()` always succeeds with one fake entry -- mirrors
/// `archive_session_lifecycle_test.rs`'s own fixture of the same name,
/// duplicated here since each test file is its own crate compile.
struct AlwaysSucceedsBackend;

impl arclain_core::ArchiveBackend for AlwaysSucceedsBackend {
    fn name(&self) -> &str {
        "always-succeeds"
    }
    fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
        arclain_core::archive::BackendCapabilities::read_only()
    }
    fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
        Ok(arclain_core::archive::ArchiveKind::Zip)
    }
    fn list(
        &self,
        _path: &Path,
        _password: Option<&str>,
    ) -> anyhow::Result<arclain_core::archive::ArchiveInfo> {
        Ok(arclain_core::archive::ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: arclain_core::archive::ArchiveKind::Zip,
            entries: vec![arclain_core::archive::ArchiveEntry {
                path: "a.txt".to_string(),
                size: 1,
                packed_size: 1,
                modified: None,
                is_dir: false,
                encrypted: false,
                crc32: None,
            }],
            encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
        })
    }
    fn extract_all(&self, _: &Path, _: &Path, _: Option<&str>) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_files(
        &self,
        _: &Path,
        _: &Path,
        _: &[String],
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn extract_directory(
        &self,
        _: &Path,
        _: &Path,
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn recompress_7z(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_files(&self, _: &Path, _: &[PathBuf]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn create_archive(&self, _: &Path, _: &[PathBuf], _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn read_text_file(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
    fn delete_files(&self, _: &Path, _: &[String]) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn add_or_update_file_from_str(&self, _: &Path, _: &str, _: &str) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn convert_to_7z(&self, _: &arclain_core::Archive, _: &Path, _: &Path) -> anyhow::Result<()> {
        unimplemented!()
    }
    fn crc32_of_entry(&self, _: &Path, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        unimplemented!()
    }
}

fn bootstrap_always_succeeds_app(temp: &tempfile::TempDir) -> arclain_app::ArclainApp {
    let paths = arclain_app::AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(AlwaysSucceedsBackend);
    arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: Some(backend),
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap must succeed against a bare temp-dir AppPaths")
}

async fn wait_for_open_completion(
    app: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = app.operation(operation_id).await.unwrap();
        if let arclain_app::event::OperationState::Completed {
            result: arclain_app::event::OperationResult::ArchiveOpened { snapshot },
        } = snapshot.state
        {
            return snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "open did not complete within the test deadline"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// 1. A plugin's metadata write, through the installed bridge, with an
/// archive already open and its tab already stamped, lands on that tab
/// only once the session-event consumer processes the resulting
/// `SessionEvent` -- not synchronously as a side effect of the trait
/// call itself (the write and the UI-facing push are now two separate
/// steps; that separation is the entire point of the swap).
#[test]
fn metadata_write_with_an_open_archive_lands_on_the_correct_tab_via_the_session_event() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_always_succeeds_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: temp.path().join("fixture.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let snapshot = wait_for_open_completion(&app, operation_id).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
        snapshot.session_id
    });
    assert_eq!(
        tab.archive_session_id.get(),
        Some(session_id),
        "sanity: the tab must already be stamped before this test's own scenario begins"
    );

    // Obtain the exact production bridge `init.rs` installs -- the
    // fallback must never run in this scenario (a session is active).
    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: a session is active"));

    runtime.block_on(async {
        bridge.set_session_metadata(
            session_id.into_raw(),
            Some(serde_json::json!({"title": "demo"})),
        );
    });

    assert_eq!(
        tab.metadata.get(),
        None,
        "the trait call alone must not touch any UI signal -- only the session store"
    );

    runtime.block_on(arclain_ui::core::operation_bridge::handle_session_event(
        shared.signals(),
        &app,
        SessionEvent::MetadataChanged { session_id },
    ));

    assert_eq!(
        tab.metadata.get(),
        Some(serde_json::json!({"title": "demo"})),
        "the session-event consumer must land the write on the tab stamped with this session"
    );
}

/// 2. With no archive session active at all, `set_active_tab_metadata`
/// must not silently drop the write -- it runs the fallback closure
/// supplied at bridge-construction time, exactly like `init.rs`'s own
/// `AppSignals`-writing closure does in production. No session event is
/// involved at all here: there is no session id to attach one to.
#[test]
fn no_session_fallback_still_lands_on_the_active_tab() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_always_succeeds_app(&temp);
    let shared = create_test_shared_state();
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let fallback_signals = shared.signals().clone();
    let bridge = app.active_tab_bridge(move |metadata| {
        fallback_signals.tabs.get().active().metadata.set(metadata);
    });

    // No `set_active_archive_session` call at all -- mirrors a plugin
    // panel emitting metadata before or without any archive open.
    runtime.block_on(async {
        bridge.set_active_tab_metadata(Some(serde_json::json!({"panel": "no archive open"})));
    });

    assert_eq!(
        tab.metadata.get(),
        Some(serde_json::json!({"panel": "no archive open"})),
        "the fallback must write to the active tab even with no facade session at all"
    );
}

/// 3. A `SessionEvent` for a session no tab is stamped with yet is
/// buffered, not dropped -- and the pre-existing drain-on-stamp
/// mechanism (`handle_open_archive_completed`, reached here through the
/// public `register_operation`) still picks it up the moment the tab is
/// stamped, exactly mirroring the pre-swap bridge's own race-closing
/// behavior for a plugin's `OnArchiveOpen` handler calling back before
/// the tab is stamped.
#[test]
fn buffered_delivery_for_a_not_yet_stamped_tab_is_drained_once_the_tab_is_stamped() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_always_succeeds_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    let (operation_id, session_id) = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: temp.path().join("fixture.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let snapshot = wait_for_open_completion(&app, operation_id).await;
        (operation_id, snapshot.session_id)
    });

    // The tab is deliberately NOT yet registered/stamped -- simulating a
    // plugin's `OnArchiveOpen` handler calling back before the operation
    // bridge gets around to stamping the originating tab.
    assert_eq!(tab.archive_session_id.get(), None);

    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: the session exists"));
    runtime.block_on(async {
        bridge.set_session_metadata(session_id.into_raw(), Some(serde_json::json!({"early": true})));
        arclain_ui::core::operation_bridge::handle_session_event(
            shared.signals(),
            &app,
            SessionEvent::MetadataChanged { session_id },
        )
        .await;
    });

    assert_eq!(
        shared
            .signals()
            .pending_session_metadata
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned(),
        Some(Some(serde_json::json!({"early": true}))),
        "with no tab stamped yet, the metadata must be buffered, not dropped"
    );

    // Now the tab gets stamped, through the real production path.
    runtime.block_on(async {
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
    });

    assert_eq!(tab.archive_session_id.get(), Some(session_id));
    assert_eq!(
        tab.metadata.get(),
        Some(serde_json::json!({"early": true})),
        "the buffered metadata must be drained onto the tab the moment it is stamped"
    );
    assert!(
        shared
            .signals()
            .pending_session_metadata
            .lock()
            .unwrap()
            .get(&session_id)
            .is_none(),
        "a drained entry must not still sit in the buffer"
    );
}

/// 4. A lagged session-event subscriber must still catch up:
/// reconciliation re-fetches every open tab's current session snapshot
/// directly, so a metadata write whose own event was dropped is still
/// picked up -- without ever having handled the live `SessionEvent` for
/// it.
#[test]
fn a_lagged_session_event_consumer_reconciles_via_archive_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_always_succeeds_app(&temp);
    let mut shared = create_test_shared_state();
    shared.facade = Some(app.clone());
    let runtime = shared.services.tokio_runtime.clone();

    let tab = shared.signals().tabs.get().active().clone();
    let tab_id = tab.id;

    let session_id = runtime.block_on(async {
        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: temp.path().join("fixture.zip"),
                password: None,
            })
            .await
            .expect("start_open_archive must be accepted");
        let snapshot = wait_for_open_completion(&app, operation_id).await;
        arclain_ui::core::operation_bridge::register_operation(&shared, operation_id, tab_id).await;
        snapshot.session_id
    });
    assert_eq!(tab.archive_session_id.get(), Some(session_id));

    let bridge = app.active_tab_bridge(|_| panic!("fallback must not run: a session is active"));

    // The write happens, but -- unlike the other tests -- its own
    // `SessionEvent` is never handled here, simulating a dropped/lagged
    // event on the broadcast channel.
    runtime.block_on(async {
        bridge.set_session_metadata(
            session_id.into_raw(),
            Some(serde_json::json!({"reconciled": true})),
        );
    });
    assert_eq!(
        tab.metadata.get(),
        None,
        "sanity: nothing has told this tab about the write yet"
    );

    runtime.block_on(
        arclain_ui::core::operation_bridge::reconcile_session_events_after_lag(
            shared.signals(),
            &app,
            1,
        ),
    );

    assert_eq!(
        tab.metadata.get(),
        Some(serde_json::json!({"reconciled": true})),
        "reconciliation must re-fetch the tab's session directly, not rely on the dropped event"
    );
}
