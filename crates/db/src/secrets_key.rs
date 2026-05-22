//! 32-byte AES key storage with zeroize-on-drop semantics.
//!
//! Extracted out of `lib.rs` (audit module-org callout) so the
//! 80-LOC key-management code doesn't share a file with the database
//! re-export bookkeeping.

use anyhow::{anyhow, Context, Result};
use arclain_app_fs::{ensure_owner_dir, restrict_owner_file};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::fs;
use std::path::Path;
use zeroize::Zeroizing;

/// Zeroizing in-memory holder for the 32-byte AES encryption key
pub struct SecretsKey(pub Zeroizing<Vec<u8>>);

// Custom Debug implementation to avoid logging key material
impl std::fmt::Debug for SecretsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsKey")
            .field("len", &self.0.len())
            .field("data", &"[REDACTED]")
            .finish()
    }
}

impl SecretsKey {
    /// Generate a new random 32-byte key
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = vec![0u8; 32];
        rand::rng().fill_bytes(&mut key);
        Self(Zeroizing::new(key))
    }

    /// Save the key to a file in base64 format with secure permissions
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        validate_path(path)?;

        if let Some(parent) = path.parent() {
            ensure_owner_dir(parent)?;
        }

        let encoded = B64.encode(&*self.0);
        fs::write(path, encoded).with_context(|| format!("writing key to {}", path.display()))?;
        restrict_owner_file(path)?;

        Ok(())
    }

    /// Load the key from a file
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("reading key from {}", path.display()))?;
        let bytes = B64
            .decode(contents.trim())
            .context("Invalid base64 in key file")?;

        if bytes.len() != 32 {
            return Err(anyhow!("Invalid key length: expected 32 bytes"));
        }

        Ok(Self(Zeroizing::new(bytes)))
    }

    /// Get the 32-byte key as a fixed-size array
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&self.0);
        arr
    }

    /// Return hex string (for debugging/logging)
    pub fn as_hex_upper(&self) -> String {
        hex_encode_upper(&self.0)
    }
}

/// Validate that a path is safe (no parent traversal)
fn validate_path(path: &Path) -> Result<()> {
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow!("Invalid path: contains parent directory traversal"));
        }
    }
    Ok(())
}

fn hex_encode_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &v in bytes {
        s.push(HEX[(v >> 4) as usize] as char);
        s.push(HEX[(v & 0x0F) as usize] as char);
    }
    s
}
