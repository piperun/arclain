//! Regressions for plugin UI work escaping the egui render thread.

mod common;

use arclain_app::error::ApplicationErrorKind;
use arclain_ui::features::plugins::application::{
    process_plugin_ui_results, request_plugin_snapshot, PluginNavigation, PluginUiJobs,
    PluginUiRequest, PluginUiResult,
};
use arclain_ui::features::plugins::domain::types::{
    PluginInfo, PluginStatus, PluginsListState, SnapshotStatus,
};
use arclain_ui::features::plugins::PluginsFeature;
use arclain_ui::features::settings::types::SettingsAction;
use arclain_ui::features::settings::SettingsFeature;
use arclain_ui::shared::image_assets::{ImageAssetState, ImageOwner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn build_wirt_install_fixture(root: &std::path::Path) -> std::path::PathBuf {
    let package_path = root.join("facade-test-fixture.wirt");
    std::fs::write(
        &package_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugins/tests/fixtures/wirt/facade-test-fixture.wirt"
        )),
    )
    .expect("write maintained package fixture");
    package_path
}

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
fn facade_plugin_queries_need_no_legacy_plugin_manager() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let jobs = PluginUiJobs::new(
        shared.facade.clone(),
        shared.services.tokio_runtime.handle().clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);

    loop {
        let _ = jobs.drain();
        if let Some(result) = jobs.plugin_snapshot() {
            assert!(
                result
                    .expect("the facade plugin read must succeed")
                    .is_empty(),
                "the isolated facade has no installed plugins"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the facade plugin snapshot never completed"
        );
        std::thread::sleep(Duration::from_millis(10));
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

    arclain_ui::features::plugins::presentation::document_dispatch::apply_navigation(
        &shared,
        "plugin",
        origin_tab,
        PluginNavigation::ClosePage,
    );

    assert!(
        !shared.image_assets.contains(key),
        "page close left its image owner alive until incidental reconciliation"
    );
}

/// The facade-rendered dialog's own close route. Replaces the deleted
/// `create_dialog_callback` test of the same shape: a `CloseDialog`
/// button used to reach the host as the reserved event id
/// `"__dialog_close"`, and now arrives as a typed
/// [`PluginNavigation::CloseDialog`] resolved by the renderer.
#[test]
fn plugin_dialog_close_navigation_releases_its_exact_image_owner() {
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

    arclain_ui::features::plugins::presentation::document_dispatch::apply_navigation(
        &shared,
        "plugin",
        origin_tab,
        PluginNavigation::CloseDialog,
    );

    assert!(
        !shared.signals().plugin_dialog_state.get().has_open_dialog(),
        "close navigation must clear the open-dialog entry the session is keyed on"
    );
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
            quarantine_state: arclain_app::plugins::PluginQuarantineState::Clear,
            last_reason: None,
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
                &shared,
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
                Some(&shared),
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
fn duplicate_facade_snapshot_requests_are_coalesced() {
    let (_temp, shared) = common::create_test_shared_state_with_facade();
    let jobs = PluginUiJobs::new(
        shared.facade.clone(),
        shared.services.tokio_runtime.handle().clone(),
    );
    let first = jobs.request(PluginUiRequest::Snapshot);
    let duplicate = jobs.request(PluginUiRequest::Snapshot);
    assert_eq!(duplicate, first, "identical pending work must coalesce");

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
fn repeated_install_requests_are_distinct_side_effects() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let jobs = PluginUiJobs::new(None, runtime.handle().clone());
    let package_path = std::path::PathBuf::from("plugin.wirt");
    let expected_fingerprint = "ab".repeat(32);

    let first = jobs.request(PluginUiRequest::InstallPackage {
        package_path: package_path.clone(),
        expected_fingerprint: expected_fingerprint.clone(),
    });
    let repeated = jobs.request(PluginUiRequest::InstallPackage {
        package_path,
        expected_fingerprint,
    });

    assert_ne!(
        first, repeated,
        "repeated install clicks are side effects and must not coalesce"
    );
}

#[test]
fn package_inspection_failure_returns_to_the_permission_review_state() {
    let (temp, shared) = common::create_test_shared_state_with_facade();
    let mut settings = SettingsFeature::new(&shared);
    let mut plugins = PluginsFeature::new(&shared);
    let package_path = temp.path().join("missing.wirt");
    let starting_epoch = shared.plugin_ui_jobs.completion_signal().get();

    settings.handle_action(
        SettingsAction::InspectPluginPackage {
            package_path: package_path.clone(),
        },
        &shared,
        Some(&mut plugins.settings_list_state),
    );

    let pending = plugins
        .settings_list_state
        .pending_install
        .as_ref()
        .expect("the selected package must own a review state immediately");
    assert_eq!(pending.package_path, package_path);
    assert!(pending.loading);
    assert!(pending.error.is_none());

    let deadline = Instant::now() + Duration::from_secs(2);
    while shared.plugin_ui_jobs.completion_signal().get() == starting_epoch {
        assert!(
            Instant::now() < deadline,
            "package inspection failure did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    process_plugin_ui_results(&shared, &mut plugins);

    let pending = plugins
        .settings_list_state
        .pending_install
        .as_ref()
        .expect("a failed inspection must keep the review dialog open");
    assert_eq!(pending.package_path, package_path);
    assert!(!pending.loading);
    assert!(pending.preview.is_none());
    assert_eq!(
        pending.error_kind,
        Some(ApplicationErrorKind::Backend),
        "the facade's stable package failure class must reach the dialog"
    );
    assert!(
        pending
            .error
            .as_deref()
            .is_some_and(|error| !error.is_empty()),
        "the bounded facade failure must be shown in the dialog"
    );
}

#[test]
fn approved_package_completes_the_real_install_and_invalidates_plugin_views() {
    let (temp, shared) = common::create_test_shared_state_with_facade();
    let package_path = build_wirt_install_fixture(temp.path());
    let mut settings = SettingsFeature::new(&shared);
    let mut plugins = PluginsFeature::new(&shared);

    settings.handle_action(
        SettingsAction::InspectPluginPackage {
            package_path: package_path.clone(),
        },
        &shared,
        Some(&mut plugins.settings_list_state),
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    let fingerprint = loop {
        process_plugin_ui_results(&shared, &mut plugins);
        let pending = plugins
            .settings_list_state
            .pending_install
            .as_ref()
            .expect("inspection must retain its review state");
        if let Some(preview) = pending.preview.as_ref() {
            break preview.fingerprint.clone();
        }
        assert!(
            pending.error.is_none(),
            "the maintained package failed inspection: {:?}",
            pending.error
        );
        assert!(Instant::now() < deadline, "package inspection timed out");
        std::thread::sleep(Duration::from_millis(10));
    };

    plugins.list_state.snapshot_status = SnapshotStatus::Ready;
    plugins.settings_list_state.snapshot_status = SnapshotStatus::Ready;
    let starting_epoch = shared
        .signals()
        .plugin_list_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    let starting_toasts = shared.toaster.lock().len();

    settings.handle_action(
        SettingsAction::ApprovePluginPackage {
            package_path: package_path.clone(),
            expected_fingerprint: fingerprint,
        },
        &shared,
        Some(&mut plugins.settings_list_state),
    );
    assert!(
        plugins
            .settings_list_state
            .pending_install
            .as_ref()
            .is_some_and(|pending| pending.installing),
        "approval must enter a non-dismissible install state"
    );

    let install_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        process_plugin_ui_results(&shared, &mut plugins);
        match plugins.settings_list_state.pending_install.as_ref() {
            None => break,
            Some(pending) => assert!(
                pending.error.is_none(),
                "the maintained package failed installation: {:?}",
                pending.error
            ),
        }
        assert!(
            Instant::now() < install_deadline,
            "package installation timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(plugins.list_state.snapshot_status, SnapshotStatus::Idle);
    assert_eq!(
        plugins.settings_list_state.snapshot_status,
        SnapshotStatus::Idle
    );
    assert_eq!(
        shared
            .signals()
            .plugin_list_epoch
            .load(std::sync::atomic::Ordering::Relaxed),
        starting_epoch + 1,
        "successful installation must invalidate independent plugin views"
    );
    assert_eq!(
        shared.toaster.lock().len(),
        starting_toasts + 1,
        "successful installation must notify the user"
    );
    let installed = shared.services.tokio_runtime.block_on(async {
        shared
            .facade
            .as_ref()
            .expect("test facade")
            .plugins()
            .await
            .expect("read installed plugins")
    });
    assert!(
        installed
            .iter()
            .any(|plugin| plugin.id == "facade-test-fixture"),
        "the approved package must be visible through the production facade"
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
    let jobs = PluginUiJobs::new(None, runtime.handle().clone());
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
fn plugin_domain_access_never_blocks_or_mutates_services_during_render() {
    let detail = include_str!("../src/features/plugins/presentation/views/detail_view.rs");
    let coordinator = include_str!("../src/features/plugins/application/ui_jobs.rs");
    let domain_section = detail
        .split_once("fn fetch_whitelist_entries")
        .expect("detail view must keep the whitelist reader")
        .1
        .split_once("/// Render the selected plugin's own configuration UI")
        .expect("domain rendering must remain before plugin UI rendering")
        .0;

    for forbidden in [
        ".block_on(",
        "services.config_service",
        "services.domain_whitelist",
    ] {
        assert!(
            !domain_section.contains(forbidden),
            "plugin detail still performs domain work during render: {forbidden}",
        );
    }
    assert!(
        coordinator.contains("set_plugin_domain_approved"),
        "domain approval must run through the bounded facade coordinator",
    );
}

#[test]
fn facade_query_coordinator_has_no_raw_plugin_event_path() {
    let coordinator = include_str!("../src/features/plugins/application/ui_jobs.rs");
    let presentation = include_str!("../src/features/plugins/presentation/mod.rs");

    assert!(
        !coordinator.contains("PluginManager")
            && !coordinator.contains("UiEvent")
            && !coordinator.contains("ReactiveUiEvent"),
        "the facade query coordinator must not regain a raw plugin-runtime event path"
    );
    assert!(
        !presentation.contains("pub mod controllers") && !presentation.contains("pub mod dispatch"),
        "the deleted legacy action controller/dispatcher must not be re-exported"
    );
}
