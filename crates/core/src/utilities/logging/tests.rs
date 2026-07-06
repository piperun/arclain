use super::*;

#[test]
fn test_logging_init() {
    // This may fail if already initialized, which is fine for tests
    let _ = init_logging();
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
