use anyhow::Result;
use redb::Database;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Generic wrapper for Redb database handling thread-safe access
#[derive(Clone)]
pub struct ReDb {
    db: Arc<Mutex<Database>>,
}

impl ReDb {
    /// Open or create a Redb database at the specified path
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Execute a closure with access to the underlying database
    pub fn with_connection<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Database) -> Result<R>,
    {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        f(&*db)
    }
}
