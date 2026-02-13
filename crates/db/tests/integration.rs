//! Integration Tests for arclain_db
//!
//! Tests pool/connection management and MetadataStore construction.
//!
//! Note: Metadata CRUD tests have been removed — metadata operations are now
//! handled by `gameta_database::DieselBackend` through `LibraryService`,
//! and are tested in gameta's own test suite.

use arclain_db::{CacheDb, DieselPool, MetadataStore, SqliteDb};
use diesel::prelude::*;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

fn setup_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp dir")
}

// =============================================================================
// POOL / CONNECTION TESTS
// =============================================================================

mod pool_tests {
    use super::*;

    #[test]
    fn test_pool_exhaustion_recovery() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("pool_exhaust.db");

        let pool = DieselPool::new(&db_path).unwrap();

        // Get many connections in sequence (not holding them)
        for _ in 0..100 {
            let conn = pool.get();
            assert!(conn.is_ok(), "Pool should provide connections");
            // conn dropped here, returned to pool
        }
    }

    #[test]
    fn test_pool_connection_validity() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("pool_valid.db");

        let pool = DieselPool::new(&db_path).unwrap();

        // Get and use connection
        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("SELECT 1")
                .execute(&mut *conn)
                .expect("Connection should be valid");
        }

        // Get another connection
        {
            let mut conn = pool.get().unwrap();
            diesel::sql_query("SELECT 1")
                .execute(&mut *conn)
                .expect("Connection should still be valid");
        }
    }

    #[test]
    fn test_concurrent_pool_access() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("concurrent_pool.db");

        let pool = Arc::new(DieselPool::new(&db_path).unwrap());

        let mut handles = vec![];
        for _ in 0..10 {
            let pool = Arc::clone(&pool);
            let handle = thread::spawn(move || {
                for _ in 0..10 {
                    let mut conn = pool.get().expect("Should get connection");
                    diesel::sql_query("SELECT 1")
                        .execute(&mut *conn)
                        .expect("Query should succeed");
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread panicked");
        }
    }
}

// =============================================================================
// METADATA STORE CONSTRUCTION TESTS
// =============================================================================

mod store_tests {
    use super::*;

    #[test]
    fn test_metadata_store_construction() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("store.db");

        let sqlite_db = SqliteDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let _store = MetadataStore::new(
            sqlite_db,
            pool,
            temp_dir.path().to_path_buf(),
            Some(db_path),
        );

        // Store should be constructed without errors (schema init, migration)
    }

    #[test]
    fn test_metadata_store_reopen() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("reopen.db");

        // First session
        {
            let sqlite_db = SqliteDb::open(&db_path).unwrap();
            let pool = DieselPool::new(&db_path).unwrap();
            let _store = MetadataStore::new(
                sqlite_db,
                pool,
                temp_dir.path().to_path_buf(),
                Some(db_path.clone()),
            );
        }

        // Second session — should not error on re-opening
        {
            let sqlite_db = SqliteDb::open(&db_path).unwrap();
            let pool = DieselPool::new(&db_path).unwrap();
            let _store = MetadataStore::new(
                sqlite_db,
                pool,
                temp_dir.path().to_path_buf(),
                Some(db_path),
            );
        }
    }

    #[test]
    fn test_cache_operations_after_construction() {
        let temp_dir = setup_temp_dir();
        let db_path = temp_dir.path().join("cache_ops.db");

        // Use CacheDb::open which initializes all schemas (including cache_index)
        let cache_db = CacheDb::open(&db_path).unwrap();
        let pool = DieselPool::new(&db_path).unwrap();
        let store = MetadataStore::new(
            cache_db.into_sqlite_db(),
            pool,
            temp_dir.path().to_path_buf(),
            Some(db_path),
        );

        // Cache operations should work after construction
        let stats = store.get_cache_stats();
        assert!(stats.is_ok());

        let hashes = store.get_all_content_hashes();
        assert!(hashes.is_ok());
        assert!(hashes.unwrap().is_empty());
    }
}
