use crate::cache;
use crate::library;
use crate::{DieselPool, SqliteDb};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// The MetadataStore manages persistent metadata storage.
///
/// It wraps the SQLite database (metadata.sqlite) and provides:
/// - Schema migration from arclain's old format to gameta's format
/// - Cache index operations for binary content tracking
///
/// Product metadata CRUD is handled by `LibraryService`'s diesel backend,
/// which the `gameta` feature supplies.
#[derive(Clone)]
pub struct MetadataStore {
    db: SqliteDb,
    pool: DieselPool,
    #[allow(dead_code)] // Reserved for future asset storage
    root_path: PathBuf,
}

impl MetadataStore {
    pub fn new(
        db: SqliteDb,
        pool: DieselPool,
        root_path: PathBuf,
        db_path: Option<PathBuf>,
    ) -> Result<Self> {
        // Run schema migration (old arclain → gameta format)
        db.with_connection(|conn| {
            match library::migration::migrate_to_gameta_schema(conn, db_path.as_deref())? {
                library::migration::MigrationResult::NotNeeded => {}
                library::migration::MigrationResult::Migrated { total, converted } => {
                    tracing::info!(
                        "[MetadataStore] Schema migration: {}/{} rows converted",
                        converted,
                        total
                    );
                }
            }
            Ok(())
        })
        .context("migrate product_metadata schema")?;

        // Ensure gameta product_metadata table exists (idempotent)
        db.with_connection(|conn| library::migration::ensure_gameta_product_metadata_schema(conn))
            .context("ensure product_metadata schema")?;

        // Initialize product_content table (arclain-specific, not in gameta)
        db.with_connection(|conn| {
            conn.execute_batch(library::content::CREATE_TABLE_SQL)?;
            Ok(())
        })
        .context("initialize product_content schema")?;

        // Drop legacy table if it exists
        if let Err(e) = db.with_connection(|conn| {
            conn.execute("DROP TABLE IF EXISTS dlsite_metadata_cache", [])?;
            Ok(())
        }) {
            tracing::warn!("Failed to drop legacy dlsite_metadata_cache table: {}", e);
        }

        let store = Self {
            db,
            pool,
            root_path,
        };

        // Clean up old search cache entries (search results shouldn't be cached)
        match store.delete_all_search_cache() {
            Ok(deleted) => {
                if deleted > 0 {
                    tracing::info!(
                        "[MetadataStore] Cleaned up {} search cache entries",
                        deleted
                    );
                }
            }
            Err(e) => {
                tracing::warn!("[MetadataStore] Failed to clean search cache: {}", e);
            }
        }

        Ok(store)
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }

    pub fn pool(&self) -> &DieselPool {
        &self.pool
    }

    /// Clear the cache index (legacy support)
    pub fn clear_cache_index(&self) -> Result<()> {
        self.pool.with_conn(|conn| cache::clear_all_entries(conn))
    }

    /// Get cache statistics
    pub fn get_cache_stats(&self) -> Result<cache::CacheStats> {
        self.pool.with_conn(|conn| cache::get_cache_stats(conn))
    }

    /// Delete orphaned cache entries (entries with product_id not in product_metadata)
    pub fn delete_orphaned_cache_entries(&self) -> Result<usize> {
        self.pool
            .with_conn(|conn| cache::delete_orphaned_entries(conn))
    }

    /// Delete search cache entries older than specified days
    pub fn delete_old_search_cache(&self, days: i64) -> Result<usize> {
        self.pool
            .with_conn(|conn| cache::delete_old_search_cache(conn, days))
    }

    /// Fix cache entries: update cache_type and product_id based on key patterns
    pub fn migrate_fix_cache_entries(&self) -> Result<(usize, usize)> {
        self.pool.with_conn(|conn| cache::migrate_fix_entries(conn))
    }

    /// Get all content hashes in the cache index
    pub fn get_all_content_hashes(&self) -> Result<Vec<String>> {
        self.pool
            .with_conn(|conn| cache::get_all_content_hashes(conn))
    }

    /// Delete all search cache entries (search results are ephemeral)
    pub fn delete_all_search_cache(&self) -> Result<usize> {
        self.pool
            .with_conn(|conn| cache::delete_all_search_cache(conn))
    }
}
