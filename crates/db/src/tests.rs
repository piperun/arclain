//! Comprehensive tests for database operations and security features

use crate::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper to create a temporary test directory
fn setup_test_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

/// Helper to create test database paths
fn test_db_paths(temp_dir: &TempDir) -> (PathBuf, PathBuf) {
    let config_path = temp_dir.path().join("config.sqlite");
    let secrets_path = temp_dir.path().join("secrets").join("pass.sqlite");
    (config_path, secrets_path)
}

/// Generate a random 32-byte key for testing
fn generate_test_key() -> SecretsKey {
    use rand::RngCore;
    let mut key = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    SecretsKey(zeroize::Zeroizing::new(key))
}

#[test]
fn test_secrets_key_generation() {
    // Test that keys can be generated and are different
    let key1 = generate_test_key();
    let key2 = generate_test_key();

    // Keys should be different
    assert_ne!(&key1.0[..], &key2.0[..]);
    
    // Keys should be 32 bytes
    assert_eq!(key1.0.len(), 32);
    assert_eq!(key2.0.len(), 32);
}

#[test]
fn test_secrets_key_hex_encoding() {
    let key = generate_test_key();
    
    // Encode to hex
    let hex = key.as_hex_upper();
    
    // Should be 64 characters (32 bytes * 2)
    assert_eq!(hex.len(), 64);
    
    // Should only contain hex digits
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_secrets_key_debug_redaction() {
    let key = generate_test_key();
    let debug_output = format!("{:?}", key);
    
    // Debug output should not contain the actual key bytes
    assert!(debug_output.contains("REDACTED"));
    assert!(debug_output.contains("len"));
}

#[test]
fn test_config_db_creation() {
    let temp_dir = setup_test_dir();
    let (config_path, _) = test_db_paths(&temp_dir);
    
    // Create config database
    let conn = open_config_db(&config_path)
        .expect("Failed to open config DB");
    
    // Initialize schema
    init_config_schema(&conn)
        .expect("Failed to initialize schema");
    
    // Verify file exists
    assert!(config_path.exists());
    
    // Verify schema - should be able to query the table
    conn.execute("SELECT key, value FROM app_config LIMIT 0", [])
        .expect("Schema not properly initialized");
}

#[test]
fn test_secrets_db_creation_and_encryption() {
    let temp_dir = setup_test_dir();
    let secrets_path = temp_dir.path().join("secrets").join("pass.redb");
    
    // Create secrets directory
    fs::create_dir_all(secrets_path.parent().unwrap())
        .expect("Failed to create secrets dir");
    
    // Generate key
    let key = generate_test_key();
    
    // Create secrets database
    let db = SecretsDb::open(&secrets_path, &key.as_bytes())
        .expect("Failed to open secrets DB");
    
    // Verify file exists
    assert!(secrets_path.exists());
    
    // Add a rule to verify encryption
    let rules = vec![DbPassRule {
        name: "Test".to_string(),
        pattern: "*.7z".to_string(),
        password: "secret123".to_string(),
        priority: 1,
        enabled: true,
    }];
    db.replace_all_pass_rules(&rules).unwrap();
    drop(db);
    
    // Try to open with wrong key - should fail to decrypt
    let wrong_key = generate_test_key();
    let db = SecretsDb::open(&secrets_path, &wrong_key.as_bytes()).unwrap();
    let result = db.list_pass_rules();
    assert!(result.is_err(), "Should fail to decrypt with wrong key");
}

#[test]
fn test_config_get_set() {
    let temp_dir = setup_test_dir();
    let (config_path, _) = test_db_paths(&temp_dir);
    
    let conn = open_config_db(&config_path).unwrap();
    init_config_schema(&conn).unwrap();
    
    // Set a value
    set_config(&conn, "test_key", "test_value")
        .expect("Failed to set config");
    
    // Get the value back
    let value = get_config(&conn, "test_key")
        .expect("Failed to get config");
    assert_eq!(value, Some("test_value".to_string()));
    
    // Update the value
    set_config(&conn, "test_key", "new_value")
        .expect("Failed to update config");
    
    let value = get_config(&conn, "test_key").unwrap();
    assert_eq!(value, Some("new_value".to_string()));
    
    // Non-existent key should return None
    let value = get_config(&conn, "nonexistent").unwrap();
    assert_eq!(value, None);
}

#[test]
fn test_pass_rules_crud() {
    let temp_dir = setup_test_dir();
    let secrets_path = temp_dir.path().join("secrets").join("pass.redb");
    
    fs::create_dir_all(secrets_path.parent().unwrap()).unwrap();
    let key = generate_test_key();
    let db = SecretsDb::open(&secrets_path, &key.as_bytes()).unwrap();
    
    // Create password rules
    let rules = vec![
        DbPassRule {
            name: "7z archives".to_string(),
            pattern: "*.7z".to_string(),
            password: "secret123".to_string(),
            priority: 10,
            enabled: true,
        },
        DbPassRule {
            name: "backups".to_string(),
            pattern: "backup/*.zip".to_string(),
            password: "backup_pass".to_string(),
            priority: 5,
            enabled: true,
        },
    ];
    
    // Replace all rules
    db.replace_all_pass_rules(&rules).unwrap();
    
    // List rules - should find both
    let fetched = db.list_pass_rules().unwrap();
    assert_eq!(fetched.len(), 2);
    
    // Should be sorted by priority desc
    assert_eq!(fetched[0].pattern, "*.7z");
    assert_eq!(fetched[0].priority, 10);
    assert_eq!(fetched[1].pattern, "backup/*.zip");
    assert_eq!(fetched[1].priority, 5);
}

#[test]
fn test_pass_rule_debug_redaction() {
    let rule = DbPassRule {
        name: "test".to_string(),
        pattern: "*.7z".to_string(),
        password: "supersecret123".to_string(),
        priority: 1,
        enabled: true,
    };
    
    let debug_output = format!("{:?}", rule);
    
    // Password should be redacted
    assert!(!debug_output.contains("supersecret"));
    assert!(debug_output.contains("REDACTED"));
    
    // Pattern should still be visible
    assert!(debug_output.contains("*.7z"));
}

// Rekey is not applicable with redb - would need to decrypt all data with old key
// and re-encrypt with new key. This is a TODO for future implementation.

#[test]
fn test_move_vault() {
    let temp_dir = setup_test_dir();
    let source_path = temp_dir.path().join("source").join("pass.redb");
    let dest_path = temp_dir.path().join("dest").join("pass.redb");
    
    // Create source directory and database
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    let key = generate_test_key();
    let db = SecretsDb::open(&source_path, &key.as_bytes()).unwrap();
    
    // Add test data
    let rules = vec![DbPassRule {
        name: "rar".to_string(),
        pattern: "*.rar".to_string(),
        password: "rar_pass".to_string(),
        priority: 1,
        enabled: true,
    }];
    db.replace_all_pass_rules(&rules).unwrap();
    drop(db);
    
    // Move vault (simple file copy for redb)
    fs::create_dir_all(dest_path.parent().unwrap()).unwrap();
    fs::copy(&source_path, &dest_path).unwrap();
    
    // Destination should exist and be readable
    assert!(dest_path.exists());
    
    let db = SecretsDb::open(&dest_path, &key.as_bytes()).unwrap();
    let fetched = db.list_pass_rules().unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].password, "rar_pass");
}

#[test]
#[cfg(unix)]
fn test_file_permissions_unix() {
    use std::os::unix::fs::PermissionsExt;
    
    let temp_dir = setup_test_dir();
    let (config_path, secrets_path) = test_db_paths(&temp_dir);
    
    // Create config database
    let conn = open_config_db(&config_path).unwrap();
    init_config_schema(&conn).unwrap();
    drop(conn);
    
    // Check config file permissions (should be 600)
    let metadata = fs::metadata(&config_path).unwrap();
    let mode = metadata.permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "Config DB should have 600 permissions");
    
    // Create secrets database
    fs::create_dir_all(secrets_path.parent().unwrap()).unwrap();
    let key = generate_test_key();
    let conn = open_secrets_db(&secrets_path, &key).unwrap();
    init_secrets_schema(&conn).unwrap();
    drop(conn);
    
    // Check secrets file permissions (should be 600)
    let metadata = fs::metadata(&secrets_path).unwrap();
    let mode = metadata.permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "Secrets DB should have 600 permissions");
    
    // Check secrets directory permissions (should be 700)
    let dir_metadata = fs::metadata(secrets_path.parent().unwrap()).unwrap();
    let dir_mode = dir_metadata.permissions().mode();
    assert_eq!(dir_mode & 0o777, 0o700, "Secrets dir should have 700 permissions");
}

#[test]
fn test_database_integration() {
    let temp_dir = setup_test_dir();
    let config_path = temp_dir.path().join("config.sqlite");
    let secrets_path = temp_dir.path().join("secrets").join("pass.redb");
    
    // Setup databases
    fs::create_dir_all(secrets_path.parent().unwrap()).unwrap();
    let key = generate_test_key();
    
    let paths = DbPaths {
        config_db: config_path.clone(),
        secrets_db: secrets_path.clone(),
        key_file: None,
    };
    
    let dbs = open_databases(&paths, &key)
        .expect("Failed to open databases");
    
    // Store config
    set_config(&dbs.config, "theme", "dark").unwrap();
    set_config(&dbs.config, "crc_policy", "on_open").unwrap();
    
    // Store password rules
    let rules = vec![
        DbPassRule {
            name: "work".to_string(),
            pattern: "work/*.7z".to_string(),
            password: "work_pass".to_string(),
            priority: 10,
            enabled: true,
        },
        DbPassRule {
            name: "personal".to_string(),
            pattern: "personal/*.zip".to_string(),
            password: "personal_pass".to_string(),
            priority: 5,
            enabled: true,
        },
    ];
    dbs.secrets.replace_all_pass_rules(&rules).unwrap();
    
    // Verify data persistence by closing and reopening
    drop(dbs);
    
    let dbs = open_databases(&paths, &key).unwrap();
    
    // Check config
    assert_eq!(get_config(&dbs.config, "theme").unwrap(), Some("dark".to_string()));
    assert_eq!(get_config(&dbs.config, "crc_policy").unwrap(), Some("on_open".to_string()));
    
    // Check password rules
    let fetched = dbs.secrets.list_pass_rules().unwrap();
    assert_eq!(fetched.len(), 2);
    
    let work_rule = fetched.iter().find(|r| r.pattern.contains("work")).unwrap();
    assert_eq!(work_rule.password, "work_pass");
    
    let personal_rule = fetched.iter().find(|r| r.pattern.contains("personal")).unwrap();
    assert_eq!(personal_rule.password, "personal_pass");
}

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread;
    
    let temp_dir = setup_test_dir();
    let (config_path, _) = test_db_paths(&temp_dir);
    
    let conn = Arc::new(Mutex::new(open_config_db(&config_path).unwrap()));
    init_config_schema(&conn.lock().unwrap()).unwrap();
    
    let mut handles = vec![];
    
    // Spawn multiple threads writing different keys
    // Using a mutex since SQLite connections aren't thread-safe by default
    for i in 0..5 {
        let conn = Arc::clone(&conn);
        let handle = thread::spawn(move || {
            let key = format!("key{}", i);
            let value = format!("value{}", i);
            let conn = conn.lock().unwrap();
            set_config(&conn, &key, &value).unwrap();
        });
        handles.push(handle);
    }
    
    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Verify all values were written
    let conn = conn.lock().unwrap();
    for i in 0..5 {
        let key = format!("key{}", i);
        let expected = format!("value{}", i);
        let actual = get_config(&conn, &key).unwrap().unwrap();
        assert_eq!(actual, expected);
    }
}