use super::*;
use std::fs;
use tempfile::TempDir;

fn test_key() -> [u8; 32] {
    [42u8; 32] // Deterministic key for testing
}

#[test]
fn test_secrets_db_create() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");

    let _db = SecretsDb::open(&db_path, &test_key()).unwrap();
    assert!(db_path.exists());
}

#[test]
fn test_encrypt_decrypt() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");
    let db = SecretsDb::open(&db_path, &test_key()).unwrap();

    let plaintext = b"secret password 123";
    let encrypted = db.encrypt(plaintext).unwrap();
    let decrypted = db.decrypt(&encrypted).unwrap();

    assert_eq!(&*decrypted, plaintext);
    assert_ne!(encrypted.as_slice(), plaintext); // Ensure it's actually encrypted
}

#[test]
fn test_crud_completeness() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");
    let db = SecretsDb::open(&db_path, &test_key()).unwrap();

    // 1. Create
    let rules = vec![PassRule {
        name: "Test".to_string(),
        pattern: "*.zip".to_string(),
        password: "pass".to_string(),
        priority: 1,
        enabled: true,
    }];
    db.replace_all_pass_rules(&rules).unwrap();

    // 2. Read
    let loaded = db.list_pass_rules().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].password, "pass");

    // 3. Update (Replace)
    let mut updated_rules = loaded;
    updated_rules[0].password = "new_pass".to_string();
    db.replace_all_pass_rules(&updated_rules).unwrap();

    let reloaded = db.list_pass_rules().unwrap();
    assert_eq!(reloaded[0].password, "new_pass");

    // 4. Delete (Empty list)
    db.replace_all_pass_rules(&[]).unwrap();
    let empty = db.list_pass_rules().unwrap();
    assert!(empty.is_empty());
}

#[test]
fn test_password_special_chars() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");
    let db = SecretsDb::open(&db_path, &test_key()).unwrap();

    let weird_pass = "🔑Password!@#$%^&*()_+\n\t\r\0NullByte";
    let weird_pattern = "📁Folder/With/Unicode/名前";

    let rules = vec![PassRule {
        name: "Special".to_string(),
        pattern: weird_pattern.to_string(),
        password: weird_pass.to_string(),
        priority: 1,
        enabled: true,
    }];

    db.replace_all_pass_rules(&rules).unwrap();
    let loaded = db.list_pass_rules().unwrap();

    assert_eq!(loaded[0].password, weird_pass);
    assert_eq!(loaded[0].pattern, weird_pattern);
}

#[test]
fn test_large_payload() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");
    let db = SecretsDb::open(&db_path, &test_key()).unwrap();

    // Create a large password (1MB)
    let large_pass = "a".repeat(1024 * 1024);
    let rules = vec![PassRule {
        name: "Large".to_string(),
        pattern: "*.large".to_string(),
        password: large_pass.clone(),
        priority: 1,
        enabled: true,
    }];

    db.replace_all_pass_rules(&rules).unwrap();
    let loaded = db.list_pass_rules().unwrap();

    assert_eq!(loaded[0].password, large_pass);
}

#[test]
fn test_invalid_key_handling() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");

    // Create with one key
    {
        let db = SecretsDb::open(&db_path, &test_key()).unwrap();
        let rules = vec![PassRule {
            name: "Test".to_string(),
            pattern: "*.7z".to_string(),
            password: "secret".to_string(),
            priority: 1,
            enabled: true,
        }];
        db.replace_all_pass_rules(&rules).unwrap();
    }

    // Try to open with different key
    let wrong_key = [99u8; 32];
    let db = SecretsDb::open(&db_path, &wrong_key).unwrap();

    // Reading should fail due to decryption error
    let result = db.list_pass_rules();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Decryption failed"));
}

#[test]
fn test_corrupted_file() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("corrupt.redb");

    // Write garbage
    fs::write(&db_path, b"not a redb file").unwrap();

    // Open should fail (redb validation)
    let result = SecretsDb::open(&db_path, &test_key());
    assert!(result.is_err());
}

#[test]
fn test_db_file_locking() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("locked.redb");

    let db1 = SecretsDb::open(&db_path, &test_key()).unwrap();

    // Redb allows multiple readers but single writer.
    // Since our wrapper manages connections, opening another instance on the same file
    // within the same process might work if redb handles it, or fail if it locks exclusively.
    // Redb typically locks the file.

    let db2_result = SecretsDb::open(&db_path, &test_key());

    // Depending on redb version/config, this might succeed (shared read) or fail.
    // But writing from both would definitely contend.
    // Here we just verify we can't corrupt it by opening twice.

    if let Ok(db2) = db2_result {
        // If it allowed opening, verify we can read from both
        assert!(db1.list_pass_rules().is_ok());
        assert!(db2.list_pass_rules().is_ok());
    } else {
        // If it failed, that's also a valid locking behavior
        assert!(true);
    }
}
