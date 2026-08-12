mod support;

use arclain_app::settings::{CacheMaintenanceReport, CacheMaintenanceTask};
use arclain_app::{ArclainApp, BootstrapConfig};

fn bootstrap_app(temp: &tempfile::TempDir) -> ArclainApp {
    let paths = support::temp_paths(temp.path());
    let sevenzip = support::create_dummy_executable(temp.path(), "7zz");
    support::seed_working_sevenzip_config(&paths, &sevenzip);
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
        initial_plugin_network_routing: None,
    })
    .expect("bootstrap cache-maintenance facade")
}

#[test]
fn cache_maintenance_is_facade_owned_and_reports_each_task() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let runtime = tokio::runtime::Runtime::new().unwrap();

    assert_eq!(
        runtime
            .block_on(app.maintain_cache(CacheMaintenanceTask::ClearIndex))
            .unwrap(),
        CacheMaintenanceReport::IndexCleared
    );
    assert_eq!(
        runtime
            .block_on(app.maintain_cache(CacheMaintenanceTask::GarbageCollect))
            .unwrap(),
        CacheMaintenanceReport::OrphansRemoved { entries: 0 }
    );
    assert_eq!(
        runtime
            .block_on(app.maintain_cache(CacheMaintenanceTask::CleanOldSearch))
            .unwrap(),
        CacheMaintenanceReport::OldSearchEntriesRemoved { entries: 0 }
    );
    assert_eq!(
        runtime
            .block_on(app.maintain_cache(CacheMaintenanceTask::RepairEntries))
            .unwrap(),
        CacheMaintenanceReport::EntriesRepaired {
            cache_types: 0,
            product_ids: 0,
        }
    );
}

#[test]
fn clear_content_removes_only_the_live_content_cache_directories() {
    let temp = tempfile::tempdir().unwrap();
    let app = bootstrap_app(&temp);
    let content_dir = app.paths().cache_dir.join("content-v2");
    let partial_dir = app.paths().cache_dir.join(".partial");
    let resources_dir = app.paths().cache_dir.join("resources");
    std::fs::create_dir_all(&content_dir).unwrap();
    std::fs::create_dir_all(&partial_dir).unwrap();
    std::fs::create_dir_all(&resources_dir).unwrap();
    std::fs::write(content_dir.join("blob"), b"cached").unwrap();
    std::fs::write(partial_dir.join("pending"), b"partial").unwrap();
    std::fs::write(resources_dir.join("keep"), b"resource").unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    assert_eq!(
        runtime
            .block_on(app.maintain_cache(CacheMaintenanceTask::ClearContent))
            .unwrap(),
        CacheMaintenanceReport::ContentCleared
    );

    assert!(!content_dir.exists());
    assert!(!partial_dir.exists());
    assert!(resources_dir.join("keep").is_file());
}
