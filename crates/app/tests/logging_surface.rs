//! Contract tests for frontend startup logging.
//!
//! A frontend must be able to initialize Arclain's tracing subscriber and
//! discover the exact files its log viewer should tail without importing
//! `arclain_core::utilities`.

use arclain_app::logging;
use arclain_app::AppPaths;

#[test]
fn logging_initialization_uses_the_application_owned_log_paths() {
    let temp = tempfile::tempdir().expect("create temp profile");
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };

    let log_paths = logging::initialize(&paths).expect("initialize logging");

    assert_eq!(log_paths.app_log_path, paths.current_app_log_file());
    assert_eq!(log_paths.plugin_log_dir, paths.plugin_log_dir());
    assert!(log_paths.app_log_path.exists());
}
