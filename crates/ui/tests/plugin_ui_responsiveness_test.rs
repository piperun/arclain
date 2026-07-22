//! Regressions for plugin UI work escaping the egui render thread.

mod common;

use arclain_core::UserConfig;
use arclain_plugins::PluginManager;
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{PluginUiJobs, PluginUiRequest, PluginUiResult};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[test]
fn request_returns_while_manager_is_blocked_and_duplicate_is_coalesced() {
    let plugins_dir = tempfile::tempdir().expect("create plugin test directory");
    let manager = Arc::new(Mutex::new(
        PluginManager::new(plugins_dir.path().to_path_buf(), HashMap::new())
            .expect("create empty plugin manager"),
    ));

    let mut shared = common::create_test_shared_state();
    Arc::get_mut(&mut shared.services)
        .expect("test services must be uniquely owned")
        .plugin_manager = Some(manager.clone());
    shared.plugin_ui_jobs =
        PluginUiJobs::new(Some(manager.clone()), shared.services.tokio_runtime.clone());

    let manager_guard = manager.lock();
    let jobs = shared.plugin_ui_jobs.clone();
    let requester = jobs.clone();
    let (sent, received) = mpsc::channel();
    let request_thread = std::thread::spawn(move || {
        let first = requester.request(PluginUiRequest::Snapshot {
            user_config: UserConfig::default(),
        });
        let duplicate = requester.request(PluginUiRequest::Snapshot {
            user_config: UserConfig::default(),
        });
        sent.send((first, duplicate)).expect("send request ids");
    });

    let (first, duplicate) = received
        .recv_timeout(Duration::from_millis(100))
        .expect("request must not wait for the manager mutex");
    assert_eq!(duplicate, first, "identical pending work must coalesce");
    request_thread.join().expect("request thread must finish");

    drop(manager_guard);

    let deadline = Instant::now() + Duration::from_secs(2);
    let results = loop {
        let results = jobs.drain();
        if !results.is_empty() {
            break results;
        }
        assert!(Instant::now() < deadline, "worker did not finish snapshot");
        std::thread::sleep(Duration::from_millis(10));
    };

    assert_eq!(results.len(), 1, "coalesced work must produce one result");
    assert!(matches!(
        &results[0],
        PluginUiResult::SnapshotLoaded { request_id, .. } if *request_id == first
    ));
}

#[test]
fn invalidation_rejects_a_late_snapshot_result() {
    let plugins_dir = tempfile::tempdir().expect("create plugin test directory");
    let manager = Arc::new(Mutex::new(
        PluginManager::new(plugins_dir.path().to_path_buf(), HashMap::new())
            .expect("create empty plugin manager"),
    ));
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(Some(manager.clone()), runtime);
    let manager_guard = manager.lock();
    let starting_epoch = jobs.completion_signal().get();

    jobs.request(PluginUiRequest::Snapshot {
        user_config: UserConfig::default(),
    });
    jobs.invalidate_plugin_snapshots();
    drop(manager_guard);

    let deadline = Instant::now() + Duration::from_secs(2);
    while jobs.completion_signal().get() == starting_epoch {
        assert!(
            Instant::now() < deadline,
            "invalidated worker request did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        jobs.drain().is_empty(),
        "an invalidated request must not publish a stale result"
    );
    assert!(
        jobs.plugin_snapshot(&UserConfig::default()).is_none(),
        "late result repopulated the invalidated snapshot cache"
    );
}

#[test]
fn reopening_the_same_page_uses_a_new_initialization_generation() {
    let plugins_dir = tempfile::tempdir().expect("create plugin test directory");
    let manager = Arc::new(Mutex::new(
        PluginManager::new(plugins_dir.path().to_path_buf(), HashMap::new())
            .expect("create empty plugin manager"),
    ));
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(Some(manager.clone()), runtime);
    let manager_guard = manager.lock();

    let first = jobs.request(PluginUiRequest::PageInit {
        plugin_id: "plugin".to_string(),
        page_id: "page".to_string(),
        origin_tab: TabId(1),
    });
    let reopened = jobs.request(PluginUiRequest::PageInit {
        plugin_id: "plugin".to_string(),
        page_id: "page".to_string(),
        origin_tab: TabId(1),
    });

    assert_ne!(
        first, reopened,
        "page lifecycle generations must never coalesce"
    );
    drop(manager_guard);
}

#[test]
fn opposite_enable_mutations_finish_in_request_order() {
    let plugins_dir = tempfile::tempdir().expect("create plugin test directory");
    let wasm_path = plugins_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .expect("write plugin fixture");
    let mut plugin_manager = PluginManager::new(plugins_dir.path().join("plugins"), HashMap::new())
        .expect("create plugin manager");
    plugin_manager
        .install_plugin(&wasm_path)
        .expect("install plugin fixture");
    let manager = Arc::new(Mutex::new(plugin_manager));
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(Some(manager.clone()), runtime);
    let manager_guard = manager.lock();

    jobs.request(PluginUiRequest::SetEnabled {
        plugin_id: "ui-demo".to_string(),
        enabled: false,
    });
    jobs.request(PluginUiRequest::SetEnabled {
        plugin_id: "ui-demo".to_string(),
        enabled: true,
    });
    drop(manager_guard);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut completed = 0;
    while completed < 2 {
        completed += jobs
            .drain()
            .into_iter()
            .filter(|result| matches!(result, PluginUiResult::MutationFinished { .. }))
            .count();
        assert!(Instant::now() < deadline, "mutations did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        manager.lock().is_plugin_enabled("ui-demo"),
        "the later enable request must deterministically win"
    );
}

#[test]
fn render_sources_never_lock_or_block_on_plugin_manager_work() {
    let sources = [
        (
            "plugin page",
            include_str!("../src/features/plugins/presentation/pages/plugins_page.rs"),
        ),
        (
            "plugin detail",
            include_str!("../src/features/plugins/presentation/views/detail_view.rs"),
        ),
        (
            "plugin settings",
            include_str!("../src/features/plugins/presentation/views/settings_view.rs"),
        ),
        (
            "plugin rendering",
            include_str!("../src/features/plugins/presentation/views/rendering.rs"),
        ),
        (
            "archive properties",
            include_str!(
                "../src/features/archive_browser/presentation/components/properties_panel.rs"
            ),
        ),
        (
            "archive panel",
            include_str!("../src/features/archive_browser/presentation/components/panel.rs"),
        ),
        (
            "archive page",
            include_str!("../src/features/archive_browser/presentation/views/browser_page.rs"),
        ),
        (
            "toolbar",
            include_str!("../src/shared/components/toolbar/mod.rs"),
        ),
        ("app chrome", include_str!("../src/core/app_rendering.rs")),
        (
            "layout editor",
            include_str!("../src/features/settings/presentation/pages/layout_editor/mod.rs"),
        ),
        (
            "settings feature",
            include_str!("../src/features/settings/presentation/feature.rs"),
        ),
        (
            "content render",
            include_str!("../src/core/arclain_app/content_handler.rs"),
        ),
    ];
    let forbidden = [
        "plugin_manager.as_ref().map(|m| m.lock())",
        "manager_arc.lock()",
        "plugin_manager.try_lock()",
        "dispatch_plugin_event_blocking",
    ];

    for (name, source) in sources {
        for pattern in forbidden {
            assert!(
                !source.contains(pattern),
                "{name} still contains render-time plugin work: {pattern}",
            );
        }
    }
}
