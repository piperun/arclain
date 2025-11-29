//! Encrypted password storage using redb + AES-256-GCM
//! Pure Rust implementation with no OpenSSL dependencies

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::Zeroizing;

const PASS_RULES_TABLE: TableDefinition<u32, &[u8]> = TableDefinition::new("pass_rules");
const METADATA_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

/// Encrypted password rule stored in redb
#[derive(Clone, Serialize, Deserialize)]
pub struct PassRule {
    pub name: String,
    pub pattern: String,
    #[serde(skip)]
    pub password: String, // Not serialized - encrypted separately
    pub priority: u32,
    pub enabled: bool,
}

impl std::fmt::Debug for PassRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassRule")
            .field("name", &self.name)
            .field("pattern", &self.pattern)
            .field("password", &"[REDACTED]")
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Encrypted password database using redb + AES-256-GCM
pub struct SecretsDb {
    db: Database,
    cipher: Aes256Gcm,
}

impl SecretsDb {
    /// Open or create encrypted secrets database
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self> {
        // Validate path
        if path.to_str().is_none() {
            return Err(anyhow!("Invalid UTF-8 path"));
        }

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }

        // Open/create database
        let db = Database::create(path)
            .with_context(|| format!("opening redb at {}", path.display()))?;

        // Initialize cipher with the key
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|e| anyhow!("Invalid encryption key: {}", e))?;

        // Initialize tables
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(PASS_RULES_TABLE)?;
            let _ = write_txn.open_table(METADATA_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db, cipher })
    }

    /// Encrypt data using AES-256-GCM
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        // Generate random 96-bit nonce
        let mut nonce_bytes = [0u8; 12];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        // Encrypt
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        // Prepend nonce to ciphertext (nonce is not secret)
        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt data using AES-256-GCM
    fn decrypt(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if data.len() < 12 {
            return Err(anyhow!("Invalid encrypted data: too short"));
        }

        // Extract nonce (first 12 bytes) and convert to array
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes.copy_from_slice(&data[..12]);
        let nonce = Nonce::from(nonce_bytes);

        // Decrypt remaining data
        let plaintext = self
            .cipher
            .decrypt(&nonce, &data[12..])
            .map_err(|e| anyhow!("Decryption failed: {}", e))?;

        Ok(Zeroizing::new(plaintext))
    }

    /// List all password rules
    pub fn list_pass_rules(&self) -> Result<Vec<PassRule>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PASS_RULES_TABLE)?;

        let mut rules = Vec::new();
        for item in table.iter()? {
            let (_id, encrypted_data) = item?;
            let decrypted = self.decrypt(encrypted_data.value())?;

            // Deserialize the encrypted payload
            let payload: RulePayload =
                serde_json::from_slice(&decrypted).context("Failed to deserialize rule")?;

            rules.push(PassRule {
                name: payload.name,
                pattern: payload.pattern,
                password: payload.password,
                priority: payload.priority,
                enabled: payload.enabled,
            });
        }

        // Sort by priority (desc) then name (asc)
        rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });

        Ok(rules)
    }

    /// Replace all password rules
    pub fn replace_all_pass_rules(&self, rules: &[PassRule]) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(PASS_RULES_TABLE)?;

            // Clear existing rules
            let keys: Vec<u32> = table.iter()?.map(|item| item.unwrap().0.value()).collect();
            for key in keys {
                table.remove(key)?;
            }

            // Insert new rules
            for (idx, rule) in rules.iter().enumerate() {
                let payload = RulePayload {
                    name: rule.name.clone(),
                    pattern: rule.pattern.clone(),
                    password: rule.password.clone(),
                    priority: rule.priority,
                    enabled: rule.enabled,
                };

                let json = serde_json::to_vec(&payload)?;
                let encrypted = self.encrypt(&json)?;
                table.insert(idx as u32, encrypted.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Compact the database to reclaim space
    pub fn compact(&self) -> Result<()> {
        // redb automatically compacts on close, so this is a no-op
        // but we keep the method for API compatibility
        Ok(())
    }
}

/// Internal structure for encrypted storage (includes password)
#[derive(Serialize, Deserialize)]
struct RulePayload {
    name: String,
    pattern: String,
    password: String,
    priority: u32,
    enabled: bool,
}

#[cfg(test)]
mod tests {
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
}
