use super::*;
use tempfile::TempDir;

fn test_key() -> [u8; 32] {
    [42u8; 32] // Deterministic key for testing
}

#[test]
fn test_secrets_db_create() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");

    let db = SecretsDb::open(&db_path, &test_key()).unwrap();
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
fn test_pass_rules_crud() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("secrets.redb");
    let db = SecretsDb::open(&db_path, &test_key()).unwrap();

    // Create rules
    let rules = vec![
        PassRule {
            name: "Work".to_string(),
            pattern: "work/*.7z".to_string(),
            password: "work_pass_123".to_string(),
            priority: 10,
            enabled: true,
        },
        PassRule {
            name: "Personal".to_string(),
            pattern: "personal/*.zip".to_string(),
            password: "personal_pass_456".to_string(),
            priority: 5,
            enabled: true,
        },
    ];

    // Save rules
    db.replace_all_pass_rules(&rules).unwrap();

    // Read back
    let loaded = db.list_pass_rules().unwrap();
    assert_eq!(loaded.len(), 2);

    // Should be sorted by priority (desc)
    assert_eq!(loaded[0].name, "Work");
    assert_eq!(loaded[0].priority, 10);
    assert_eq!(loaded[0].password, "work_pass_123");

    assert_eq!(loaded[1].name, "Personal");
    assert_eq!(loaded[1].priority, 5);
    assert_eq!(loaded[1].password, "personal_pass_456");
}

#[test]
fn test_wrong_key_fails() {
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
}
