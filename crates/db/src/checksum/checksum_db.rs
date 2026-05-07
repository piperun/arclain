//! Checksum database for file integrity verification

use crate::{diesel_err, SqliteDb};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

/// Verification mode for checksum storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerifyMode {
    /// Disabled - no checksums stored
    Disabled,
    /// Simple - only root hash stored
    #[default]
    Simple,
    /// Full - all file hashes stored
    Full,
}

impl VerifyMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "disabled" | "off" => Some(VerifyMode::Disabled),
            "simple" | "root" => Some(VerifyMode::Simple),
            "full" | "all" => Some(VerifyMode::Full),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VerifyMode::Disabled => "disabled",
            VerifyMode::Simple => "simple",
            VerifyMode::Full => "full",
        }
    }
}

/// Unique operation identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpId(pub String);

impl OpId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn from_string(s: String) -> Self {
        Self(s)
    }
}

impl Default for OpId {
    fn default() -> Self {
        Self::new()
    }
}

/// Type of file operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpType {
    Extract,
    Move,
    Copy,
    Organize,
}

impl OpType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpType::Extract => "extract",
            OpType::Move => "move",
            OpType::Copy => "copy",
            OpType::Organize => "organize",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "extract" => Some(OpType::Extract),
            "move" => Some(OpType::Move),
            "copy" => Some(OpType::Copy),
            "organize" => Some(OpType::Organize),
            _ => None,
        }
    }
}

/// State of an operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpState {
    Pending,
    SourceHashed,
    Copied,
    DestVerified,
    Completed,
    Failed,
}

impl OpState {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpState::Pending => "PENDING",
            OpState::SourceHashed => "SOURCE_HASHED",
            OpState::Copied => "COPIED",
            OpState::DestVerified => "DEST_VERIFIED",
            OpState::Completed => "COMPLETED",
            OpState::Failed => "FAILED",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(OpState::Pending),
            "SOURCE_HASHED" => Some(OpState::SourceHashed),
            "COPIED" => Some(OpState::Copied),
            "DEST_VERIFIED" => Some(OpState::DestVerified),
            "COMPLETED" => Some(OpState::Completed),
            "FAILED" => Some(OpState::Failed),
            _ => None,
        }
    }

    pub fn is_incomplete(&self) -> bool {
        matches!(
            self,
            OpState::Pending | OpState::SourceHashed | OpState::Copied | OpState::DestVerified
        )
    }
}

/// A tracked file operation
#[derive(Debug, Clone)]
pub struct DbOperation {
    pub id: OpId,
    pub op_type: OpType,
    pub state: OpState,
    pub source_path: PathBuf,
    pub dest_path: Option<PathBuf>,
    pub source_hash: Option<Vec<u8>>,
    pub dest_hash: Option<Vec<u8>>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Stored file checksum
#[derive(Debug, Clone)]
pub struct DbFileChecksum {
    pub path: String,
    pub archive_id: Option<String>,
    pub hash: Vec<u8>,
    pub size: u64,
    pub algorithm: String,
}

/// Checksum database wrapper
pub struct ChecksumDb {
    db: SqliteDb,
}

impl ChecksumDb {
    /// Open the checksum database
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let db = SqliteDb::open(path)?;
        db.init_schema(Self::init_schema)?;
        Ok(Self { db })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS checksum_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT OR IGNORE INTO checksum_settings (key, value) VALUES ('algorithm', 'crc32');
            INSERT OR IGNORE INTO checksum_settings (key, value) VALUES ('mode', 'simple');

            CREATE TABLE IF NOT EXISTS file_checksums (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                archive_id TEXT,
                hash BLOB NOT NULL,
                size INTEGER NOT NULL,
                mtime INTEGER,
                algorithm TEXT NOT NULL,
                computed_at INTEGER NOT NULL,
                UNIQUE(path, archive_id)
            );
            CREATE INDEX IF NOT EXISTS idx_checksum_path ON file_checksums(path);

            CREATE TABLE IF NOT EXISTS merkle_roots (
                id INTEGER PRIMARY KEY,
                archive_id TEXT UNIQUE,
                root_hash BLOB NOT NULL,
                file_count INTEGER NOT NULL,
                algorithm TEXT NOT NULL,
                computed_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS checksum_operations (
                op_id TEXT PRIMARY KEY,
                op_type TEXT NOT NULL,
                state TEXT NOT NULL,
                source_path TEXT NOT NULL,
                dest_path TEXT,
                source_hash BLOB,
                dest_hash BLOB,
                error_message TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_checksum_op_state ON checksum_operations(state);

            CREATE TABLE IF NOT EXISTS verification_log (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL,
                archive_id TEXT,
                expected_hash BLOB NOT NULL,
                actual_hash BLOB NOT NULL,
                matched INTEGER NOT NULL,
                verified_at INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    /// Get inner SqliteDb
    pub fn into_sqlite_db(self) -> SqliteDb {
        self.db
    }

    /// Execute with connection
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        self.db.with_connection(f)
    }
}

// === Database Operations ===

pub fn get_checksum_algorithm(conn: &Connection) -> Result<String> {
    let algo: String = conn.query_row(
        "SELECT value FROM checksum_settings WHERE key = 'algorithm'",
        [],
        |row| row.get(0),
    )?;
    Ok(algo)
}

pub fn set_checksum_algorithm(conn: &Connection, algo: &str) -> Result<()> {
    conn.execute(
        "UPDATE checksum_settings SET value = ?1 WHERE key = 'algorithm'",
        [algo],
    )?;
    Ok(())
}

pub fn get_checksum_mode(conn: &Connection) -> Result<VerifyMode> {
    let mode: String = conn.query_row(
        "SELECT value FROM checksum_settings WHERE key = 'mode'",
        [],
        |row| row.get(0),
    )?;
    Ok(VerifyMode::from_str(&mode).unwrap_or_default())
}

pub fn set_checksum_mode(conn: &Connection, mode: VerifyMode) -> Result<()> {
    conn.execute(
        "UPDATE checksum_settings SET value = ?1 WHERE key = 'mode'",
        [mode.as_str()],
    )?;
    Ok(())
}

pub fn store_file_checksum(
    conn: &Connection,
    path: &str,
    archive_id: Option<&str>,
    hash: &[u8],
    size: u64,
    algorithm: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    conn.execute(
        r#"INSERT INTO file_checksums (path, archive_id, hash, size, mtime, algorithm, computed_at)
           VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)
           ON CONFLICT(path, archive_id) DO UPDATE SET
               hash = excluded.hash, size = excluded.size, computed_at = excluded.computed_at"#,
        params![path, archive_id, hash, size as i64, algorithm, now],
    )?;
    Ok(())
}

pub fn get_file_checksum(
    conn: &Connection,
    path: &str,
    archive_id: Option<&str>,
) -> Result<Option<DbFileChecksum>> {
    let result: Option<(Vec<u8>, i64, String)> = conn
        .query_row(
            "SELECT hash, size, algorithm FROM file_checksums WHERE path = ?1 AND archive_id IS ?2",
            params![path, archive_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    Ok(result.map(|(hash, size, algorithm)| DbFileChecksum {
        path: path.to_string(),
        archive_id: archive_id.map(|s| s.to_string()),
        hash,
        size: size as u64,
        algorithm,
    }))
}

pub fn store_merkle_root(
    conn: &Connection,
    archive_id: &str,
    root_hash: &[u8],
    file_count: usize,
    algorithm: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    conn.execute(
        r#"INSERT INTO merkle_roots (archive_id, root_hash, file_count, algorithm, computed_at)
           VALUES (?1, ?2, ?3, ?4, ?5)
           ON CONFLICT(archive_id) DO UPDATE SET
               root_hash = excluded.root_hash, file_count = excluded.file_count, computed_at = excluded.computed_at"#,
        params![archive_id, root_hash, file_count as i64, algorithm, now],
    )?;
    Ok(())
}

pub fn get_merkle_root(conn: &Connection, archive_id: &str) -> Result<Option<Vec<u8>>> {
    let result: Option<Vec<u8>> = conn
        .query_row(
            "SELECT root_hash FROM merkle_roots WHERE archive_id = ?1",
            [archive_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(result)
}

pub fn begin_checksum_operation(conn: &Connection, op: &DbOperation) -> Result<()> {
    conn.execute(
        r#"INSERT INTO checksum_operations 
           (op_id, op_type, state, source_path, dest_path, source_hash, dest_hash, error_message, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
        params![
            op.id.0,
            op.op_type.as_str(),
            op.state.as_str(),
            op.source_path.to_string_lossy(),
            op.dest_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            op.source_hash,
            op.dest_hash,
            op.error_message,
            op.created_at,
            op.updated_at,
        ],
    )?;
    Ok(())
}

pub fn update_checksum_operation(conn: &Connection, op: &DbOperation) -> Result<()> {
    conn.execute(
        r#"UPDATE checksum_operations SET state = ?2, source_hash = ?3, dest_hash = ?4, error_message = ?5, updated_at = ?6
           WHERE op_id = ?1"#,
        params![
            op.id.0,
            op.state.as_str(),
            op.source_hash,
            op.dest_hash,
            op.error_message,
            op.updated_at,
        ],
    )?;
    Ok(())
}

pub fn get_pending_checksum_operations(conn: &Connection) -> Result<Vec<DbOperation>> {
    let mut stmt = conn.prepare(
        r#"SELECT op_id, op_type, state, source_path, dest_path, source_hash, dest_hash, error_message, created_at, updated_at
           FROM checksum_operations WHERE state NOT IN ('COMPLETED', 'FAILED') ORDER BY created_at ASC"#,
    )?;

    let ops = stmt
        .query_map([], |row| {
            let op_type_str: String = row.get(1)?;
            let state_str: String = row.get(2)?;
            let source_path_str: String = row.get(3)?;
            let dest_path_str: Option<String> = row.get(4)?;

            Ok(DbOperation {
                id: OpId::from_string(row.get(0)?),
                op_type: OpType::from_str(&op_type_str).unwrap_or(OpType::Copy),
                state: OpState::from_str(&state_str).unwrap_or(OpState::Pending),
                source_path: PathBuf::from(source_path_str),
                dest_path: dest_path_str.map(PathBuf::from),
                source_hash: row.get(5)?,
                dest_hash: row.get(6)?,
                error_message: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(ops)
}

pub fn delete_checksum_operation(conn: &Connection, op_id: &OpId) -> Result<()> {
    conn.execute(
        "DELETE FROM checksum_operations WHERE op_id = ?1",
        [&op_id.0],
    )?;
    Ok(())
}

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// Get checksum algorithm using Diesel DSL
pub fn get_checksum_algorithm_diesel(conn: &mut diesel::SqliteConnection) -> Result<String> {
    use crate::diesel_schema::checksum_settings::dsl::*;
    use diesel::result::OptionalExtension;

    let result = checksum_settings
        .filter(key.eq("algorithm"))
        .select(value)
        .first::<String>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result.unwrap_or_else(|| "blake3".to_string()))
}

/// Set checksum algorithm using Diesel DSL
pub fn set_checksum_algorithm_diesel(
    conn: &mut diesel::SqliteConnection,
    algo: &str,
) -> Result<()> {
    use crate::diesel_schema::checksum_settings::dsl::*;

    diesel::insert_into(checksum_settings)
        .values((key.eq("algorithm"), value.eq(algo)))
        .on_conflict(key)
        .do_update()
        .set(value.eq(algo))
        .execute(conn)
        .map_err(diesel_err("insert"))?;

    Ok(())
}

/// Get checksum mode using Diesel DSL
pub fn get_checksum_mode_diesel(conn: &mut diesel::SqliteConnection) -> Result<VerifyMode> {
    use crate::diesel_schema::checksum_settings::dsl::*;
    use diesel::result::OptionalExtension;

    let result = checksum_settings
        .filter(key.eq("mode"))
        .select(value)
        .first::<String>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result
        .and_then(|s| VerifyMode::from_str(&s))
        .unwrap_or_default())
}

/// Set checksum mode using Diesel DSL
pub fn set_checksum_mode_diesel(
    conn: &mut diesel::SqliteConnection,
    mode: VerifyMode,
) -> Result<()> {
    use crate::diesel_schema::checksum_settings::dsl::*;

    diesel::insert_into(checksum_settings)
        .values((key.eq("mode"), value.eq(mode.as_str())))
        .on_conflict(key)
        .do_update()
        .set(value.eq(mode.as_str()))
        .execute(conn)
        .map_err(diesel_err("insert"))?;

    Ok(())
}
