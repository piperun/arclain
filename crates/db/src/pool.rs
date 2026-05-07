//! Connection pool management for Diesel
//!
//! Provides thread-safe connection pooling using diesel::r2d2.

use anyhow::{Context, Result};
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection};
use diesel::SqliteConnection;
use std::path::Path;
use std::sync::Arc;

/// Type alias for a pooled Diesel SQLite connection
pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;
pub type DbConn = PooledConnection<ConnectionManager<SqliteConnection>>;

/// Wrapper around r2d2 connection pool for Diesel
#[derive(Clone)]
pub struct DieselPool {
    pool: Arc<DbPool>,
}

impl DieselPool {
    /// Create a new connection pool from a database path
    pub fn new(db_path: &Path) -> Result<Self> {
        let db_url = db_path.to_string_lossy().into_owned();
        let manager = ConnectionManager::<SqliteConnection>::new(&db_url);

        let pool = Pool::builder()
            .max_size(10)
            .min_idle(Some(1))
            .build(manager)
            .context("Failed to create database connection pool")?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    /// Create a new connection pool from a database URL string
    pub fn from_url(db_url: &str) -> Result<Self> {
        let manager = ConnectionManager::<SqliteConnection>::new(db_url);

        let pool = Pool::builder()
            .max_size(10)
            .min_idle(Some(1))
            .build(manager)
            .context("Failed to create database connection pool")?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    /// Get a connection from the pool
    pub fn get(&self) -> Result<DbConn> {
        self.pool
            .get()
            .context("Failed to get database connection from pool")
    }

    /// Execute a function with a connection from the pool
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut SqliteConnection) -> Result<T>,
    {
        let mut conn = self.get()?;
        f(&mut conn)
    }

    /// Get the underlying pool for advanced usage
    pub fn inner(&self) -> &DbPool {
        &self.pool
    }
}

impl std::fmt::Debug for DieselPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DieselPool")
            .field("max_size", &self.pool.max_size())
            .field("state", &self.pool.state())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = DieselPool::from_url(":memory:");
        assert!(pool.is_ok());
    }

    #[test]
    fn test_get_connection() {
        let pool = DieselPool::from_url(":memory:").unwrap();
        let conn = pool.get();
        assert!(conn.is_ok());
    }
}
