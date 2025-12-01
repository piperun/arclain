use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{Connection, OptionalExtension};
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

mod secrets;
pub use secrets::{PassRule as DbPassRule, SecretsDb};

mod organization;
pub use organization::{delete_rule, get_rule, list_rules, save_rule, DbOrganizationRule};

/// Re-export Connection so dependents don't need rusqlite directly.
pub use rusqlite::Connection as DbConnection;

/// Canonical paths for the two databases and optional key-file
#[derive(Debug, Clone)]
pub struct DbPaths {
    pub config_db: PathBuf,
    pub secrets_db: PathBuf,
    pub key_file: Option<PathBuf>,
}

impl DbPaths {
    /// Defaults:
    /// - config.sqlite at %APPDATA%/{app}/
    /// - pass.redb at %APPDATA%/{app}/secrets/ (redb with AES-256-GCM)
    /// - master key at %APPDATA%/{app}/secrets/master.key
    pub fn defaults(app_name: &str) -> Result<Self> {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(app_name);
        let secrets_dir = base.join("secrets");

        // Create directories with secure permissions
        fs::create_dir_all(&base)
            .with_context(|| format!("creating config dir {}", base.display()))?;
        fs::create_dir_all(&secrets_dir)
            .with_context(|| format!("creating secrets dir {}", secrets_dir.display()))?;

        // Set directory permissions to 700 (user-only) on Unix
        #[cfg(unix)]
        {
            let perms = fs::Permissions::from_mode(0o700);
            fs::set_permissions(&secrets_dir, perms)
                .with_context(|| format!("setting permissions on {}", secrets_dir.display()))?;
        }

        Ok(Self {
            config_db: base.join("config.sqlite"),
            secrets_db: secrets_dir.join("pass.redb"),
            key_file: Some(secrets_dir.join("master.key")),
        })
    }
}

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
        rand::thread_rng().fill_bytes(&mut key);
        Self(Zeroizing::new(key))
    }

    /// Save the key to a file in base64 format with secure permissions
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        // Validate path to prevent directory traversal
        validate_path(path)?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;

            // Set directory permissions to 700 (user-only) on Unix
            #[cfg(unix)]
            {
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(parent, perms)
                    .with_context(|| format!("setting permissions on {}", parent.display()))?;
            }
        }

        // Encode key as base64 and write to file
        let encoded = B64.encode(&*self.0);
        fs::write(path, encoded.as_bytes())
            .with_context(|| format!("writing key file {}", path.display()))?;

        // Set file permissions to 600 (user-only read/write) on Unix
        #[cfg(unix)]
        {
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, perms)
                .with_context(|| format!("setting permissions on {}", path.display()))?;
        }

        Ok(())
    }

    /// Load from a user-provided key file (32 bytes raw, 64-char hex, or base64)
    pub fn from_file(path: &Path) -> Result<Self> {
        // Validate path to prevent directory traversal
        validate_path(path)?;

        let data =
            fs::read(path).with_context(|| format!("reading key file {}", path.display()))?;

        // Set file permissions to 600 (user-only read/write) on Unix
        #[cfg(unix)]
        {
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, perms)
                .with_context(|| format!("setting permissions on {}", path.display()))?;
        }
        // 1) Exact 32 raw bytes
        if data.len() == 32 {
            return Ok(Self(Zeroizing::new(data)));
        }
        // 2) Try as UTF-8 text: hex or base64
        let text = std::str::from_utf8(&data)
            .map(|s| s.trim())
            .map_err(|_| anyhow!("key file is not 32 raw bytes and not UTF-8 text"))?;

        if is_hex_64(text) {
            let mut out = Vec::with_capacity(32);
            hex_decode_into(text, &mut out)?;
            if out.len() != 32 {
                return Err(anyhow!("hex key must decode to 32 bytes"));
            }
            return Ok(Self(Zeroizing::new(out)));
        }

        // base64 (strict)
        let out = B64
            .decode(text.as_bytes())
            .map_err(|_| anyhow!("key file is not valid base64 or hex"))?;
        if out.len() != 32 {
            return Err(anyhow!("base64 key must decode to 32 bytes"));
        }
        Ok(Self(Zeroizing::new(out)))
    }

    /// Get the 32-byte key as a fixed-size array
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&self.0[..32]);
        arr
    }

    /// Return hex string (for debugging/logging)
    pub fn as_hex_upper(&self) -> String {
        hex_encode_upper(&self.0)
    }
}

/// Open both databases: config (SQLite) and secrets (redb with AES encryption)
pub struct ConfigDbs {
    pub config: Connection,
    pub secrets: SecretsDb,
}

pub fn open_databases(paths: &DbPaths, key: &SecretsKey) -> Result<ConfigDbs> {
    let cfg = open_config_db(&paths.config_db)?;
    init_config_schema(&cfg)?;

    let sec = SecretsDb::open(&paths.secrets_db, &key.as_bytes())?;

    Ok(ConfigDbs {
        config: cfg,
        secrets: sec,
    })
}

/// Plain SQLite for config.sqlite
pub fn open_config_db(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("opening config db {}", path.display()))?;

    // Set file permissions to 600 (user-only read/write) on Unix
    #[cfg(unix)]
    {
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_file() {
                let perms = fs::Permissions::from_mode(0o600);
                fs::set_permissions(path, perms)
                    .with_context(|| format!("setting permissions on {}", path.display()))?;
            }
        }
    }

    // Pragmas safe for plain sqlite
    conn.execute_batch(
        "\
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        PRAGMA temp_store=MEMORY;
        ",
    )?;
    Ok(conn)
}

fn init_config_schema(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_config(
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta(
            migration INTEGER NOT NULL
        );",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS organization_rules(
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            category TEXT DEFAULT 'General',
            trigger_json TEXT NOT NULL,
            actions_json TEXT NOT NULL,
            priority INTEGER DEFAULT 0,
            is_enabled BOOLEAN DEFAULT 1,
            is_system BOOLEAN DEFAULT 0
        );",
        [],
    )?;
    // Ensure a meta row exists
    conn.execute(
        "INSERT INTO meta (migration) SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM meta);",
        [],
    )?;
    Ok(())
}

/// Simple K/V config helpers (stored in plain config.sqlite)
pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM app_config WHERE key = ?1")?;
    let val = stmt
        .query_row([key], |row| row.get::<_, String>(0))
        .optional()?;
    Ok(val)
}

pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_config(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

/// Helpers

fn is_hex_64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn hex_decode_into(s: &str, out: &mut Vec<u8>) -> Result<()> {
    fn val(c: u8) -> Result<u8> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(10 + (c - b'a')),
            b'A'..=b'F' => Ok(10 + (c - b'A')),
            _ => Err(anyhow!("invalid hex")),
        }
    }
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return Err(anyhow!("hex length must be even"));
    }
    out.clear();
    out.reserve(b.len() / 2);
    let mut i = 0usize;
    while i < b.len() {
        let hi = val(b[i])?;
        let lo = val(b[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
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

/// Validate file path to prevent directory traversal and ensure it's a valid UTF-8 path
fn validate_path(path: &Path) -> Result<()> {
    // Ensure path is valid UTF-8
    path.to_str()
        .ok_or_else(|| anyhow!("path contains invalid UTF-8: {}", path.display()))?;

    // Check for directory traversal attempts
    let path_str = path.to_string_lossy();
    if path_str.contains("..") {
        return Err(anyhow!(
            "path contains directory traversal: {}",
            path.display()
        ));
    }

    // Additional platform-specific checks
    #[cfg(windows)]
    {
        // Check for Windows reserved names
        let forbidden = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            let name_upper = file_name.to_uppercase();
            if forbidden.iter().any(|&f| name_upper.starts_with(f)) {
                return Err(anyhow!(
                    "path uses Windows reserved name: {}",
                    path.display()
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
