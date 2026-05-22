use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_standard_initialization() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    let dirs = AppDirectories::init("test_app", Some(overrides)).unwrap();

    assert!(dirs.config_dir.exists());
    assert!(dirs.cache_dir.exists());
    assert!(dirs.secrets_dir.exists());
    assert!(dirs.plugins_dir.exists());
    assert!(dirs.logs_dir.exists());
    // temp_dir comes from std::env::temp_dir(), not from the overrides,
    // so it sits outside `root` — just verify it was created.
    assert!(dirs.temp_dir.exists());

    assert!(dirs.config_dir.starts_with(&root));
    assert!(dirs.secrets_dir.starts_with(&dirs.config_dir));
}

#[test]
fn test_idempotency() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    let dirs1 = AppDirectories::init("test_app", Some(overrides.clone())).unwrap();
    let dirs2 = AppDirectories::init("test_app", Some(overrides)).unwrap();

    assert_eq!(dirs1.config_dir, dirs2.config_dir);
}

#[test]
fn test_file_conflict() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    // Plant a regular file where the app's config dir would land.
    let config_home = root.join("config_home");
    fs::create_dir_all(&config_home).unwrap();
    let conflict_path = config_home.join("test_app");
    fs::write(&conflict_path, "I am a file").unwrap();

    let overrides = PathOverrides {
        config_home: Some(config_home),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // create_dir_all on a path that already exists as a file errors;
    // the function should surface that, not silently mask it.
    let result = AppDirectories::init("test_app", Some(overrides));
    assert!(result.is_err());
}

#[test]
#[cfg(unix)]
fn test_permission_denied_unix() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let locked_parent = root.join("locked");
    fs::create_dir(&locked_parent).unwrap();

    // Strip write so child dir creation fails.
    let mut perms = fs::metadata(&locked_parent).unwrap().permissions();
    perms.set_mode(0o500);
    fs::set_permissions(&locked_parent, perms).unwrap();

    let overrides = PathOverrides {
        config_home: Some(locked_parent),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    let result = AppDirectories::init("test_app", Some(overrides));
    assert!(result.is_err());
}

#[test]
#[cfg(windows)]
fn test_locked_resource_windows() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // Windows disallows < > : " / \ | ? * in filenames. Passing an
    // app name containing these should bubble up as an error from the
    // first create_dir_all attempt.
    let invalid_app_name = "app<Val>";
    let result = AppDirectories::init(invalid_app_name, Some(overrides));

    assert!(
        result.is_err(),
        "Should fail to create directory with invalid characters"
    );
}
