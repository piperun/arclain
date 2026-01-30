//! Core LibSqlBackend struct and constructors

use anyhow::Result;
use libsql::{Builder, Connection, Database};
use std::path::Path;

/// Async libSQL-backed storage for gameta metadata
///
/// This backend uses libSQL (a SQLite fork) for async database operations.
/// It stores product metadata, content references, fetch logs, and integrity info.
pub struct LibSqlBackend {
    /// Database handle (kept alive to maintain connection)
    pub(crate) _db: Database,
    /// Active database connection
    pub(crate) conn: Connection,
}

impl LibSqlBackend {
    /// Create a new libSQL backend with a local database file
    ///
    /// # Arguments
    /// * `path` - Path to the database file (will be created if it doesn't exist)
    ///
    /// # Example
    /// ```ignore
    /// let backend = LibSqlBackend::new_local("./gameta.db").await?;
    /// backend.init_schema().await?;
    /// ```
    pub async fn new_local<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Builder::new_local(path.as_ref()).build().await?;
        let conn = db.connect()?;
        Ok(Self { _db: db, conn })
    }

    /// Create an in-memory database (useful for testing)
    ///
    /// # Example
    /// ```ignore
    /// let backend = LibSqlBackend::new_memory().await?;
    /// backend.init_schema().await?;
    /// ```
    pub async fn new_memory() -> Result<Self> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Ok(Self { _db: db, conn })
    }
}
