use crate::cache_index;
use crate::product_metadata::{self, MetadataSource, ProductMetadata};
use crate::SqliteDb;
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
    #[allow(dead_code)] // Reserved for future asset storage
    root_path: PathBuf,
}

impl MetadataStore {
    pub fn new(db: SqliteDb, root_path: PathBuf) -> Self {
        // Ensure schema exists
        if let Err(e) =
            db.with_connection(|conn| product_metadata::init_product_metadata_schema(conn))
        {
            tracing::error!("Failed to init ProductMetadata schema: {}", e);
        }

        Self { db, root_path }
    }

    /// Get metadata by ID (e.g. "dlsite:RJ123456")
    pub fn get(&self, id: &str) -> Result<Option<ProductMetadata>> {
        self.db
            .with_connection(|conn| product_metadata::load(conn, id))
    }

    /// Save metadata
    pub fn save(&self, meta: &ProductMetadata) -> Result<()> {
        self.db
            .with_connection(|conn| product_metadata::save(conn, meta))
    }

    /// List all metadata for a source
    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<ProductMetadata>> {
        self.db
            .with_connection(|conn| product_metadata::list_by_source(conn, source))
    }

    /// Delete metadata by ID
    pub fn delete(&self, id: &str) -> Result<()> {
        self.db
            .with_connection(|conn| product_metadata::delete(conn, id))
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }

    /// Clear the cache index (legacy support)
    pub fn clear_cache_index(&self) -> Result<()> {
        self.db
            .with_connection(|conn| cache_index::clear_all_entries(conn))
    }
}
