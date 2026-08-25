//! Archive-only integration proof for embedders that disable plugin hosting.
//!
//! This target is run with `--no-default-features`. It exercises the real
//! application facade against the maintained tiny RAR fixture and an isolated
//! on-disk profile, while proving that the configured plugin boundary remains
//! untouched for the entire application lifetime.

#![cfg(not(feature = "plugin-host"))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use arclain_app::archive::{
    ArchivePath, EntrySortKey, ListEntriesRequest, OpenArchiveRequest, SortDirection,
};
use arclain_app::event::{OperationResult, OperationState};
use arclain_app::settings::{ArchiveSettingsPatch, PatchValue, SettingsPatch};
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

fn foreign_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the test's foreign runtime")
}

fn temp_paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugin-boundary/plugins"),
    }
}

fn tiny_archive_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/timestamped-rar4.rar")
}

async fn wait_for_archive_opened(
    app: &ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) -> arclain_app::archive::ArchiveSnapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let operation = app
            .operation(operation_id)
            .await
            .expect("the accepted archive-open operation must remain queryable");
        match operation.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return snapshot,
            OperationState::Failed { error } => {
                panic!("the maintained tiny archive failed to open: {error:?}")
            }
            OperationState::Cancelled => panic!("the archive open was unexpectedly cancelled"),
            _ if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            _ => panic!("the archive open did not complete within the test deadline"),
        }
    }
}

fn keep_archive_patch_with_collision_policy(policy: &str) -> ArchiveSettingsPatch {
    ArchiveSettingsPatch {
        backend_mode: PatchValue::Keep,
        cache_directory: PatchValue::Keep,
        temp_directory: PatchValue::Keep,
        transfer_directory: PatchValue::Keep,
        sevenzip_path: PatchValue::Keep,
        default_collision_policy: PatchValue::Set(policy.to_owned()),
    }
}

fn bootstrap_archive_only(paths: AppPaths) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: Some(1),
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("archive-only bootstrap must succeed")
}

fn assert_plugin_boundary_untouched(paths: &AppPaths, sentinel: &Path) {
    assert_eq!(
        std::fs::read(sentinel).expect("the pre-bootstrap sentinel must remain readable"),
        b"archive-only boundary sentinel"
    );
    assert!(
        !paths.plugins_dir.exists(),
        "archive-only bootstrap must not create the configured plugins directory"
    );
    assert!(
        !paths.plugin_log_dir().exists(),
        "archive-only operation must not create a per-plugin log directory"
    );
    assert!(
        !paths.plugins_dir.join(".wirt-quarantine.json").exists(),
        "archive-only operation must not create the plugin state ledger"
    );
    assert!(
        !paths
            .plugins_dir
            .join("archive-only-sentinel.wirt")
            .exists(),
        "archive-only operation must not create a plugin package"
    );
    assert!(
        !paths
            .plugins_dir
            .join("archive-only-sentinel/package.sha256")
            .exists(),
        "archive-only operation must not create installed-package state"
    );

    let mut boundary_entries = std::fs::read_dir(
        sentinel
            .parent()
            .expect("the pre-bootstrap sentinel has a parent"),
    )
    .expect("inspect the full configured plugin boundary")
    .map(|entry| {
        entry
            .expect("read configured plugin-boundary entry")
            .file_name()
    })
    .collect::<Vec<_>>();
    boundary_entries.sort();
    assert_eq!(
        boundary_entries,
        [std::ffi::OsString::from("pre-bootstrap-sentinel")],
        "only the pre-bootstrap sentinel may exist in the configured plugin boundary"
    );
}

/// Catches archive-only bootstrap or ordinary archive/settings work reaching
/// into plugin-owned storage despite `plugin-host` being absent from the
/// compiled application.
#[test]
fn archive_only_embedding_never_touches_plugin_storage() {
    let runtime = foreign_runtime();
    let temp = tempfile::tempdir().expect("create isolated archive-only profile");
    let paths = temp_paths(temp.path());
    let plugin_boundary = paths
        .plugins_dir
        .parent()
        .expect("the configured plugins directory has a parent");
    std::fs::create_dir_all(plugin_boundary).expect("create plugin boundary parent");
    let sentinel = plugin_boundary.join("pre-bootstrap-sentinel");
    std::fs::write(&sentinel, b"archive-only boundary sentinel")
        .expect("write pre-bootstrap sentinel");

    let app = bootstrap_archive_only(paths.clone());

    assert_plugin_boundary_untouched(&paths, &sentinel);

    runtime.block_on(async {
        let initial_settings = app.settings().await.expect("load archive settings");
        let saved_settings = app
            .update_settings(SettingsPatch {
                expected_revision: initial_settings.revision,
                archive: Some(keep_archive_patch_with_collision_policy("skip")),
                network: None,
                security: None,
                general: None,
            })
            .await
            .expect("save an ordinary archive setting");
        assert_eq!(saved_settings.archive.default_collision_policy, "skip");

        let operation_id = app
            .start_open_archive(OpenArchiveRequest {
                source_path: tiny_archive_fixture(),
                password: None,
            })
            .await
            .expect("opening the maintained tiny archive must be accepted");
        let archive = wait_for_archive_opened(&app, operation_id).await;
        let entries = app
            .list_entries(
                archive.session_id,
                ListEntriesRequest {
                    directory: ArchivePath::root(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .await
            .expect("list the maintained tiny archive");
        assert_eq!(entries.entries.len(), 1);
        assert_eq!(entries.entries[0].name, "timestamped.txt");
        app.close_archive(archive.session_id)
            .await
            .expect("close the maintained tiny archive");
        app.shutdown()
            .await
            .expect("archive-only shutdown must succeed");
    });
    drop(app);

    assert_plugin_boundary_untouched(&paths, &sentinel);

    let reopened = bootstrap_archive_only(paths.clone());
    runtime.block_on(async {
        assert_eq!(
            reopened
                .settings()
                .await
                .expect("reload settings from the archive-only profile")
                .archive
                .default_collision_policy,
            "skip",
            "the archive setting must survive shutdown and a fresh bootstrap"
        );
        reopened
            .shutdown()
            .await
            .expect("reopened archive-only app must shut down cleanly");
    });
    drop(reopened);

    assert_plugin_boundary_untouched(&paths, &sentinel);
}
