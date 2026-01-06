use crate::cache;
use crate::legacy;
use crate::library::{self, MetadataSource, ProductMetadata};
use crate::{DieselPool, SqliteDb};
use anyhow::Result;
use std::path::PathBuf;

/// The MetadataStore manages persistent metadata storage.
///
/// It wraps the SQLite database (metadata.sqlite) and provides
/// access to the `ProductMetadata` table.
/// Future expansion: It will also manage a file-based asset cache.
#[derive(Clone)]
pub struct MetadataStore {
    db: SqliteDb,
    pool: DieselPool,
    #[allow(dead_code)] // Reserved for future asset storage
    root_path: PathBuf,
}

impl MetadataStore {
    pub fn new(db: SqliteDb, pool: DieselPool, root_path: PathBuf) -> Self {
        // Ensure schema exists
        // Ensure schema exists (using legacy init because it contains valid SQL)
        if let Err(e) =
            db.with_connection(|conn| legacy::metadata::init_product_metadata_schema(conn))
        {
            tracing::error!("Failed to init ProductMetadata schema: {}", e);
        }

        // Drop legacy table if it exists
        if let Err(e) = db.with_connection(|conn| {
            conn.execute("DROP TABLE IF EXISTS dlsite_metadata_cache", [])?;
            Ok(())
        }) {
            tracing::warn!("Failed to drop legacy dlsite_metadata_cache table: {}", e);
        }

        Self {
            db,
            pool,
            root_path,
        }
    }

    /// Get metadata by ID (e.g. "dlsite:RJ123456")
    pub fn get(&self, id: &str) -> Result<Option<ProductMetadata>> {
        self.pool.with_conn(|conn| library::load(conn, id))
    }

    /// Save metadata
    pub fn save(&self, meta: &ProductMetadata) -> Result<()> {
        self.pool.with_conn(|conn| library::save(conn, meta))
    }

    /// List all metadata for a source
    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<ProductMetadata>> {
        self.pool
            .with_conn(|conn| library::list_by_source(conn, source))
    }

    /// Delete metadata by ID
    pub fn delete(&self, id: &str) -> Result<()> {
        self.pool.with_conn(|conn| library::delete(conn, id))
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
}
