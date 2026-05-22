//! Encrypted password storage using redb + AES-256-GCM
//! Pure Rust implementation with no OpenSSL dependencies

use crate::redb_wrapper::ReDb;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use arclain_app_fs::{ensure_owner_dir, restrict_owner_file};
use redb::{ReadableTable, TableDefinition};
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
#[derive(Clone)]
pub struct SecretsDb {
    db: ReDb,
    cipher: Aes256Gcm,
}

impl SecretsDb {
    /// Open or create encrypted secrets database
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self> {
        // Validate path
        if path.to_str().is_none() {
            return Err(anyhow!("Invalid UTF-8 path"));
        }

        // Owner-only parent dir (0o700 on Unix). See arclain_app_fs.
        if let Some(parent) = path.parent() {
            ensure_owner_dir(parent)?;
        }

        // Open/create database using wrapper
        let db = ReDb::open(path).with_context(|| format!("opening redb at {}", path.display()))?;

        // Initialize cipher with the key
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|e| anyhow!("Invalid encryption key: {}", e))?;

        // Initialize tables
        db.with_connection(|conn| {
            let write_txn = conn.begin_write()?;
            {
                let _ = write_txn.open_table(PASS_RULES_TABLE)?;
                let _ = write_txn.open_table(METADATA_TABLE)?;
            }
            write_txn.commit()?;
            Ok(())
        })?;

        // Owner-only DB file (0o600 on Unix). See arclain_app_fs.
        restrict_owner_file(path)?;

        Ok(Self { db, cipher })
    }

    /// Encrypt data using AES-256-GCM
    pub(crate) fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        // Generate random 96-bit nonce
        let mut nonce_bytes = [0u8; 12];
        use rand::RngCore;
        rand::rng().fill_bytes(&mut nonce_bytes);
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
    pub(crate) fn decrypt(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
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
        self.db.with_connection(|conn| {
            let read_txn = conn.begin_read()?;
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
        })
    }

    /// Replace all password rules
    pub fn replace_all_pass_rules(&self, rules: &[PassRule]) -> Result<()> {
        self.db.with_connection(|conn| {
            let write_txn = conn.begin_write()?;
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
        })
    }

    /// Compact the database to reclaim space
    pub fn compact(&self) -> Result<()> {
        // redb automatically compacts on close, so this is a no-op
        // but we keep the method for API compatibility
        Ok(())
    }

    /// Get a generic secret (e.g., SOCKS5 password)
    pub fn get_secret(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        self.db.with_connection(|conn| {
            let read_txn = conn.begin_read()?;
            let table = read_txn.open_table(METADATA_TABLE)?;

            if let Some(encrypted) = table.get(key)? {
                let decrypted = self.decrypt(encrypted.value())?;
                let s = String::from_utf8(decrypted.to_vec())
                    .map_err(|e| anyhow!("Invalid UTF-8 in secret: {}", e))?;
                Ok(Some(Zeroizing::new(s)))
            } else {
                Ok(None)
            }
        })
    }

    /// Set a generic secret
    pub fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        let encrypted = self.encrypt(value.as_bytes())?;
        self.db.with_connection(|conn| {
            let write_txn = conn.begin_write()?;
            {
                let mut table = write_txn.open_table(METADATA_TABLE)?;
                table.insert(key, encrypted.as_slice())?;
            }
            write_txn.commit()?;
            Ok(())
        })
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
