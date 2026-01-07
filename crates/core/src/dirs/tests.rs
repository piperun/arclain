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
    // Temp dir is usually system temp, might not be inside our temp root unless overridden env var,
    // but our init logic uses std::env::temp_dir().
    // We didn't override env temp, so check it exists at least.
    assert!(dirs.temp_dir.exists());

    // Check paths are correct relative to root
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

    // First call
    let dirs1 = AppDirectories::init("test_app", Some(overrides.clone())).unwrap();

    // Second call - should succeed without error
    let dirs2 = AppDirectories::init("test_app", Some(overrides)).unwrap();

    assert_eq!(dirs1.config_dir, dirs2.config_dir);
}

#[test]
fn test_file_conflict() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    // Create a FILE where the config dir should be
    let config_home = root.join("config_home");
    fs::create_dir_all(&config_home).unwrap();

    let conflict_path = config_home.join("test_app");
    fs::write(&conflict_path, "I am a file").unwrap();

    let overrides = PathOverrides {
        config_home: Some(config_home),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // Should fail because it can't create directory over a file
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

    // Remove write permissions
    let mut perms = fs::metadata(&locked_parent).unwrap().permissions();
    perms.set_mode(0o500); // Read+Execute only
    fs::set_permissions(&locked_parent, perms).unwrap();

    let overrides = PathOverrides {
        config_home: Some(locked_parent),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    let result = AppDirectories::init("test_app", Some(overrides));
    assert!(result.is_err());
}

// Windows-specific lock test
#[test]
#[cfg(windows)]
fn test_locked_resource_windows() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    // We want to block creation of `root/config/app_name`.
    // Let's create a *file* there first, then open it exclusively.
    // Actually, `init` would fail just because it's a file (tested in file_conflict).
    // To test "locked resource" in a way that differs from file conflict,
    // we could try to exclusively lock the PARENT directory?
    // Or simpler: The file conflict test covers the "cannot create dir because something is there" case.
    // A true "lock" test on a directory specifically for creation is hard because `create_dir_all`
    // checks existence first.
    // If the directory ALREADY exists, `create_dir_all` succeeds.
    // So we need a scenario where we CANNOT create it.
    //
    // Let's try an invalid character test which is Windows specific.

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // Windows disallows < > : " / \ | ? * in filenames.
    // If we pass an app name with these, join might work (PathBuf is lenient),
    // but create_dir should fail.
    let invalid_app_name = "app<Val>";
    let result = AppDirectories::init(invalid_app_name, Some(overrides));

    assert!(
        result.is_err(),
        "Should fail to create directory with invalid characters"
    );
}
