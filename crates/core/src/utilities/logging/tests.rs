use super::*;
use std::fs;

#[test]
fn file_appender_initialization_uses_the_injected_directory() {
    let temp = tempfile::tempdir().unwrap();
    let log_dir = temp.path().join("logs");

    let appender = prepare_file_appender(&log_dir, "test.log").unwrap();

    assert!(log_dir.join("test.log").is_file());
    drop(appender);
}

#[test]
fn file_appender_initialization_returns_invalid_directory_errors_without_panicking() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("not-a-directory");
    fs::write(&file, b"occupied").unwrap();

    let result = std::panic::catch_unwind(|| prepare_file_appender(&file.join("logs"), "test.log"));

    assert!(result.is_ok(), "log initialization must not panic");
    assert!(result.unwrap().is_err());
}

#[test]
fn app_log_path_uses_arclain_log_name_for_date() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 7, 6).unwrap();

    let path = app_log_path_for_date(date);

    assert_eq!(path.file_name().unwrap(), "arclain-2026-07-06.log");
    assert!(path.ends_with("arclain/logs/arclain-2026-07-06.log"));
}

#[test]
fn plugin_log_dir_lives_under_app_log_dir() {
    let plugin_dir = plugin_log_dir();

    assert_eq!(plugin_dir.file_name().unwrap(), "plugins");
    assert_eq!(plugin_dir.parent().unwrap(), app_log_dir());
}
