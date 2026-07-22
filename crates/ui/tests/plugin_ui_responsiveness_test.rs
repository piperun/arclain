//! Regressions for plugin UI work escaping the egui render thread.

mod common;

use arclain_core::UserConfig;
use arclain_plugins::{types::PluginAction, PluginManager};
use arclain_ui::core::tabs::TabId;
use arclain_ui::features::plugins::application::{
    process_plugin_ui_results, request_plugin_snapshot, PluginUiJobs, PluginUiRequest,
    PluginUiResult,
};
use arclain_ui::features::plugins::domain::types::{
    PluginInfo, PluginStatus, PluginsListState, SnapshotStatus,
};
use arclain_ui::features::plugins::PluginsFeature;
use arclain_ui::shared::image_assets::{ImageAssetState, ImageOwner};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

fn wait_for_image_failure(shared: &arclain_ui::shared::SharedState, key: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !matches!(
        shared.image_assets.state(key),
        Some(ImageAssetState::Failed(_))
    ) {
        assert!(Instant::now() < deadline, "image worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn plugin_page_close_releases_its_exact_image_owner() {
    let shared = common::create_test_shared_state();
    let origin_tab = shared.signals().tabs.get().active_id();
    let owner = ImageOwner::plugin_page("plugin", "page", origin_tab);
    let key = "page-close-image";
    let mut dialog_state = shared.signals().plugin_dialog_state.get();
    dialog_state.open_page("plugin", "page", origin_tab);
    shared.signals().plugin_dialog_state.set(dialog_state);
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let mut callback = arclain_ui::features::plugins::presentation::controllers::plugin_controller::create_page_callback(
        &shared,
        "plugin".to_string(),
        origin_tab,
    );
    callback("__page_close", None);

    assert!(
        !shared.image_assets.contains(key),
        "page close left its image owner alive until incidental reconciliation"
    );
}

#[test]
fn plugin_dialog_close_releases_its_exact_image_owner() {
    let shared = common::create_test_shared_state();
    let origin_tab = shared.signals().tabs.get().active_id();
    let owner = ImageOwner::plugin_dialog("plugin", "dialog", origin_tab);
    let key = "dialog-close-image";
    let mut dialog_state = shared.signals().plugin_dialog_state.get();
    dialog_state.open_dialog("plugin", "dialog", origin_tab);
    shared.signals().plugin_dialog_state.set(dialog_state);
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let mut callback = arclain_ui::features::plugins::presentation::controllers::plugin_controller::create_dialog_callback(
        &shared,
        "plugin".to_string(),
        origin_tab,
    );
    callback("__dialog_close", None);

    assert!(
        !shared.image_assets.contains(key),
        "dialog close left its image owner alive until incidental reconciliation"
    );
}

#[test]
fn plugin_dialog_native_window_close_releases_its_exact_image_owner() {
    use egui_kittest::{kittest::Queryable as _, Harness};

    let shared = common::create_test_shared_state();
    let origin_tab = shared.signals().tabs.get().active_id();
    let owner = ImageOwner::plugin_dialog("plugin", "dialog", origin_tab);
    let key = "dialog-native-window-close-image";
    let mut dialog_state = shared.signals().plugin_dialog_state.get();
    dialog_state.open_dialog("plugin", "dialog", origin_tab);
    shared.signals().plugin_dialog_state.set(dialog_state);
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let render_shared = shared.clone();
    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(800.0, 600.0))
        .build(move |ctx| {
            arclain_ui::features::plugins::presentation::views::rendering::render_dialog(
                ctx,
                &render_shared,
            );
        });
    harness.get_by_label("Close window").click();
    harness.step();

    assert!(
        !shared.signals().plugin_dialog_state.get().has_open_dialog(),
        "native window close must close dialog state"
    );
    assert!(
        !shared.image_assets.contains(key),
        "native window close left its image owner alive until another frame"
    );
}

#[test]
fn plugin_close_dialog_action_releases_its_exact_image_owner() {
    let shared = common::create_test_shared_state();
    let origin_tab = shared.signals().tabs.get().active_id();
    let owner = ImageOwner::plugin_dialog("plugin", "dialog", origin_tab);
    let key = "dialog-action-close-image";
    let mut dialog_state = shared.signals().plugin_dialog_state.get();
    dialog_state.open_dialog("plugin", "dialog", origin_tab);
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let mut toaster = arclain_widgets::Toaster::new();
    arclain_ui::features::plugins::presentation::controllers::plugin_controller::process_plugin_actions(
        vec![PluginAction::CloseDialog],
        "plugin",
        &mut dialog_state,
        &mut toaster,
        None,
        None,
        Some(&shared),
    );

    assert!(
        !shared.image_assets.contains(key),
        "plugin CloseDialog action left its image owner alive"
    );
}

#[test]
fn plugin_settings_back_releases_the_selected_plugins_image_owner() {
    let shared = common::create_test_shared_state();
    let owner = ImageOwner::plugin_settings("plugin");
    let key = "plugin-settings-back-image";
    let mut state = PluginsListState {
        plugins: vec![PluginInfo {
            id: "plugin".to_string(),
            name: "Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            description: None,
            capabilities: Vec::new(),
            enabled: true,
            loaded: true,
            status: PluginStatus::Ready,
            error: None,
            visibility: HashMap::new(),
        }],
        selected_plugin: Some("plugin".to_string()),
        ..PluginsListState::default()
    };
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let install_clicked = std::cell::Cell::new(false);
    {
        let mut config =
            arclain_ui::features::plugins::presentation::pages::plugins_page::get_header_config(
                &mut state,
                &arclain_ui::core::SettingsPage::Plugins,
                &install_clicked,
                &shared.image_assets,
            );
        config
            .on_back
            .take()
            .expect("detail header must have a back action")();
    }

    assert_eq!(state.selected_plugin, None);
    assert!(
        !shared.image_assets.contains(key),
        "settings back left the selected plugin image owner alive"
    );
}

#[test]
fn plugin_settings_selection_change_releases_the_previous_image_owner() {
    let shared = common::create_test_shared_state();
    let owner = ImageOwner::plugin_settings("removed-plugin");
    let key = "plugin-settings-selection-change-image";
    let mut state = PluginsListState {
        selected_plugin: Some("removed-plugin".to_string()),
        ..PluginsListState::default()
    };
    shared
        .image_assets
        .request(owner, key, eframe::egui::Context::default());
    wait_for_image_failure(&shared, key);

    let ctx = eframe::egui::Context::default();
    let _ = ctx.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            arclain_ui::features::plugins::presentation::pages::plugins_page::render(
                ui,
                &shared.theme,
                &mut state,
                &shared.app_state,
                Some(&shared),
                None,
            );
        });
    });

    assert_eq!(state.selected_plugin, None);
    assert!(
        !shared.image_assets.contains(key),
        "settings selection change left the previous plugin image owner alive"
    );
}

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
fn repeated_enable_mutations_preserve_aba_request_order() {
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

    for enabled in [false, true, false] {
        jobs.request(PluginUiRequest::SetEnabled {
            plugin_id: "ui-demo".to_string(),
            enabled,
        });
    }
    drop(manager_guard);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut completed = 0;
    while completed < 3 {
        completed += jobs
            .drain()
            .into_iter()
            .filter(|result| matches!(result, PluginUiResult::MutationFinished { .. }))
            .count();
        assert!(Instant::now() < deadline, "ABA mutations did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !manager.lock().is_plugin_enabled("ui-demo"),
        "the final A request must run after B instead of coalescing with the first A"
    );
}

#[test]
fn repeated_install_requests_are_distinct_ordered_side_effects() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(None, runtime);
    let wasm_path = std::path::PathBuf::from("plugin.wasm");

    let first = jobs.request(PluginUiRequest::Install {
        wasm_path: wasm_path.clone(),
    });
    let repeated = jobs.request(PluginUiRequest::Install { wasm_path });

    assert_ne!(
        first, repeated,
        "repeated install clicks are side effects and must not coalesce"
    );
}

#[test]
fn snapshot_failure_releases_ui_pending_state() {
    let shared = common::create_test_shared_state();
    let mut plugins = PluginsFeature::new(&shared);
    let starting_epoch = shared.plugin_ui_jobs.completion_signal().get();

    request_plugin_snapshot(&shared, &mut plugins.list_state);
    assert_eq!(plugins.list_state.snapshot_status, SnapshotStatus::Pending);

    let deadline = Instant::now() + Duration::from_secs(2);
    while shared.plugin_ui_jobs.completion_signal().get() == starting_epoch {
        assert!(Instant::now() < deadline, "snapshot failure did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    process_plugin_ui_results(&shared, &mut plugins);

    assert_ne!(
        plugins.list_state.snapshot_status,
        SnapshotStatus::Pending,
        "a failed snapshot must not leave the page permanently pending"
    );
}

#[test]
fn chrome_failure_is_cached_instead_of_automatically_requeued() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(None, runtime);
    let starting_epoch = jobs.completion_signal().get();

    assert!(jobs.chrome_snapshot().is_none());
    let deadline = Instant::now() + Duration::from_secs(2);
    while jobs.completion_signal().get() == starting_epoch {
        assert!(Instant::now() < deadline, "chrome failure did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    let results = jobs.drain();
    assert_eq!(results.len(), 1);

    assert!(
        jobs.chrome_snapshot().is_some(),
        "a cached chrome failure must suppress per-frame automatic retries"
    );
}

#[test]
fn page_init_failure_becomes_terminal_visible_page_state() {
    let shared = common::create_test_shared_state();
    let origin_tab = shared.signals().tabs.get().active_id();
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let mut dialog_state = dialog_signal.get();
    dialog_state.open_page("missing-plugin", "page", origin_tab);
    dialog_signal.set(dialog_state);
    let starting_epoch = shared.plugin_ui_jobs.completion_signal().get();

    let ctx = eframe::egui::Context::default();
    let mut rendered = false;
    let _ = ctx.run(eframe::egui::RawInput::default(), |ctx| {
        rendered = arclain_ui::features::plugins::presentation::views::rendering::render_page(
            ctx, &shared,
        );
    });
    assert!(rendered);
    let deadline = Instant::now() + Duration::from_secs(2);
    while shared.plugin_ui_jobs.completion_signal().get() == starting_epoch {
        assert!(
            Instant::now() < deadline,
            "page-init failure did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut plugins = PluginsFeature::new(&shared);
    process_plugin_ui_results(&shared, &mut plugins);

    let dialog_state = dialog_signal.get();
    assert!(dialog_state.page_init_error().is_some());
    assert!(!dialog_state.page_init_pending());
    assert!(!dialog_state.page_layout_ready());
}

#[test]
fn plugin_event_actions_stay_with_the_origin_tab_after_a_switch() {
    let mut manager = PluginManager::new(
        tempfile::tempdir()
            .expect("create plugin directory")
            .keep()
            .join("plugins"),
        HashMap::new(),
    )
    .expect("create plugin manager");
    manager
        .install_plugin(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/dlsite-metadata/dlsite-metadata.wasm"
        )))
        .expect("install dlsite plugin fixture");
    let manager = Arc::new(Mutex::new(manager));
    let mut shared = common::create_test_shared_state();
    Arc::get_mut(&mut shared.services)
        .expect("test services must be uniquely owned")
        .plugin_manager = Some(manager.clone());
    shared.plugin_ui_jobs =
        PluginUiJobs::new(Some(manager.clone()), shared.services.tokio_runtime.clone());

    let origin_tab = shared.signals().tabs.get().active_id();
    let starting_epoch = shared.plugin_ui_jobs.completion_signal().get();
    let manager_guard = manager.lock();
    arclain_ui::features::plugins::presentation::dispatch::dispatch_plugin_event(
        &shared,
        "dlsite-metadata".to_string(),
        "__page_init".to_string(),
        Some("dlsite_browser".to_string()),
    );

    let later_tab = {
        let mut tabs = shared.signals().tabs.get();
        let id = tabs.open(None);
        shared.signals().tabs.set(tabs);
        id
    };
    drop(manager_guard);

    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.plugin_ui_jobs.completion_signal().get() == starting_epoch {
        assert!(Instant::now() < deadline, "plugin event did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut plugins = PluginsFeature::new(&shared);
    process_plugin_ui_results(&shared, &mut plugins);

    let tabs = shared.signals().tabs.get();
    assert_eq!(
        tabs.get(origin_tab)
            .and_then(|tab| tab.page_display_name.get()),
        Some("DLSite Browser".to_string()),
        "event actions must apply to the tab that originated the WASM call"
    );
    assert_eq!(
        tabs.get(later_tab)
            .and_then(|tab| tab.page_display_name.get()),
        None,
        "switching tabs must not redirect event actions to the later-active tab"
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

#[test]
fn legacy_dispatch_has_no_parallel_originless_completion_path() {
    let dispatch = include_str!("../src/features/plugins/presentation/dispatch.rs");
    let controller =
        include_str!("../src/features/plugins/presentation/controllers/plugin_controller.rs");
    let shared_state = include_str!("../src/shared/state.rs");
    let detail_view = include_str!("../src/features/plugins/presentation/views/detail_view.rs");
    let panel = include_str!("../src/features/archive_browser/presentation/components/panel.rs");

    for forbidden in [
        "pending_plugin_actions",
        "spawn_blocking",
        "services.plugin_manager",
        ".send_ui_event(",
    ] {
        assert!(
            !dispatch.contains(forbidden),
            "legacy dispatch still bypasses origin-aware completions: {forbidden}"
        );
    }
    assert!(
        !controller.contains(".send_ui_event("),
        "plugin controller still bypasses PluginUiJobs for follow-up events"
    );
    assert!(
        !shared_state.contains("pending_plugin_actions")
            && !detail_view.contains("pending_plugin_actions"),
        "the obsolete originless pending-action queue must not remain in the UI architecture"
    );
    assert!(
        !panel.contains("origin_tab.expect("),
        "panel dispatch must handle an unavailable context without panicking"
    );
}
