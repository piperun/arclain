use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Generic SQLite database wrapper with connection pooling support
pub struct SqliteDb {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteDb {
    /// Open or create a SQLite database at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("Failed to open SQLite database at {:?}", path.as_ref()))?;

        // Enable foreign keys and WAL mode
        // We use execute_batch because PRAGMA journal_mode returns a row,
        // and execute() might fail if it expects no rows.
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )
        .context("Failed to enable WAL mode and foreign keys")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    /// Get a cloneable reference to the connection (for sharing)
    pub fn connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Execute a schema initialization function
    /// This acquires the lock and passes the connection to the init function
    pub fn init_schema<F>(&self, init_fn: F) -> Result<()>
    where
        F: FnOnce(&Connection) -> Result<()>,
    {
        let conn = self.conn.lock().unwrap();
        init_fn(&*conn)
    }

    /// Execute a query with a closure that has access to the connection
    pub fn with_connection<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap();
        f(&*conn)
    }
}

impl Clone for SqliteDb {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_sqlite_db_open() {
        let temp_file = NamedTempFile::new().unwrap();
        let db = SqliteDb::open(temp_file.path()).unwrap();

        // Verify WAL mode is enabled
        db.with_connection(|conn| {
            let journal_mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(journal_mode.to_lowercase(), "wal");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_sqlite_db_clone() {
        let temp_file = NamedTempFile::new().unwrap();
        let db1 = SqliteDb::open(temp_file.path()).unwrap();
        let db2 = db1.clone();

        // Both should share the same connection
        db1.init_schema(|conn| {
            conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", [])?;
            Ok(())
        })
        .unwrap();

        // db2 should see the table created by db1
        db2.with_connection(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='test'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_init_schema() {
        let temp_file = NamedTempFile::new().unwrap();
        let db = SqliteDb::open(temp_file.path()).unwrap();

        db.init_schema(|conn| {
            conn.execute(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL
                )",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        // Verify table was created
        db.with_connection(|conn| {
            conn.execute("INSERT INTO users (name) VALUES (?)", ["Alice"])?;
            let name: String = conn
                .query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
                .unwrap();
            assert_eq!(name, "Alice");
            Ok(())
        })
        .unwrap();
    }
}
