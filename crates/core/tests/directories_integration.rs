use arclain_app_fs::{AppDirectories, PathOverrides};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_integration_full_lifecycle() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.join("cfg")),
        cache_home: Some(root.join("cache")),
        data_home: Some(root.join("data")),
    };

    // 1. Init
    let dirs = AppDirectories::init("arclain_integration", Some(overrides)).expect("Init failed");

    // Verify creation
    assert!(dirs.config_dir.exists());
    assert!(dirs.secrets_dir.exists());

    // 2. Write something to config
    let config_file = dirs.config_dir.join("config.toml");
    fs::write(&config_file, "content").unwrap();
    assert!(config_file.exists());

    // 3. Re-init (should be fine)
    let overrides2 = PathOverrides {
        config_home: Some(root.join("cfg")),
        cache_home: Some(root.join("cache")),
        data_home: Some(root.join("data")),
    };
    let dirs2 =
        AppDirectories::init("arclain_integration", Some(overrides2)).expect("Re-init failed");

    assert_eq!(dirs.config_dir, dirs2.config_dir);
    assert!(config_file.exists()); // Should not be deleted
}

#[test]
#[cfg(windows)]
fn test_windows_reserved_names() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // "CON", "PRN", "AUX", "NUL", "COM1"-9, "LPT1"-9 are reserved on Windows
    let reserved_names = vec!["CON", "PRN", "AUX", "NUL", "COM1"];

    for name in reserved_names {
        // Attempt to create a directory named after reserved word inside our temp config root
        let res = AppDirectories::init(name, Some(overrides.clone()));
        // Creating "...\CON" is forbidden
        assert!(res.is_err(), "Should fail for reserved name: {}", name);
    }
}

#[test]
#[cfg(unix)]
fn test_linux_deep_nesting() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();

    let overrides = PathOverrides {
        config_home: Some(root.clone()),
        cache_home: Some(root.clone()),
        data_home: Some(root.clone()),
    };

    // 255 chars is typically max filename, but path can be longer.
    // Let's try a reasonably long name that fits in filename but results in long path.
    let long_name = "a".repeat(200);

    let res = AppDirectories::init(&long_name, Some(overrides));
    assert!(res.is_ok(), "Should handle long names within OS limits");

    let dirs = res.unwrap();
    assert!(dirs.config_dir.exists());
}

#[test]
fn test_symlink_behavior() {
    // Only run if we can make symlinks (Unix or Windows dev mode)
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();

        // Target where we want config to be
        let real_config = root.join("real_config");
        fs::create_dir(&real_config).unwrap();

        // "config_home" that points to symlink
        let symlink_path = root.join("symlinked_config");
        symlink(&real_config, &symlink_path).unwrap();

        let overrides = PathOverrides {
            config_home: Some(symlink_path),
            cache_home: Some(root.clone()),
            data_home: Some(root.clone()),
        };

        // Init should follow symlink and create "app" inside "real_config"
        let dirs = AppDirectories::init("app", Some(overrides)).unwrap();

        assert!(dirs.config_dir.exists());
        // Verify canonical path check if we cared, but existence is key
    }
}
