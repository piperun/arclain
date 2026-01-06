//! Library service for product metadata and content operations
//!
//! Wraps arclain_db library functions with connection pool management.

use anyhow::Result;
use arclain_db::{
    delete as delete_metadata, delete_product_content, get_all_content, get_by_external_id,
    get_cover, get_screenshots, list_by_source, list_ids_by_source, load as load_metadata,
    save as save_metadata, save_product_content, DieselPool, MetadataSource, ProductContent,
    ProductMetadata,
};

/// Service for managing product library (metadata + content)
#[derive(Clone)]
pub struct LibraryService {
    pool: DieselPool,
}

impl LibraryService {
    /// Create a new library service with the given connection pool
    pub fn new(pool: DieselPool) -> Self {
        Self { pool }
    }

    // =========================================================================
    // Metadata Operations
    // =========================================================================

    /// Load product metadata by ID
    pub fn get_metadata(&self, product_id: &str) -> Result<Option<ProductMetadata>> {
        self.pool.with_conn(|conn| load_metadata(conn, product_id))
    }

    /// Load product metadata by external ID and source
    pub fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>> {
        self.pool
            .with_conn(|conn| get_by_external_id(conn, source, external_id))
    }

    /// Save product metadata (upsert)
    pub fn save_metadata(&self, metadata: &ProductMetadata) -> Result<()> {
        self.pool.with_conn(|conn| save_metadata(conn, metadata))
    }

    /// Delete product metadata by ID
    pub fn delete_metadata(&self, product_id: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| delete_metadata(conn, product_id))
    }

    /// List all product IDs from a specific source
    pub fn list_ids_by_source(&self, source: MetadataSource) -> Result<Vec<String>> {
        self.pool.with_conn(|conn| list_ids_by_source(conn, source))
    }

    /// List all products from a specific source
    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<ProductMetadata>> {
        self.pool.with_conn(|conn| list_by_source(conn, source))
    }

    // =========================================================================
    // Content Operations
    // =========================================================================

    /// Get all content for a product
    pub fn get_all_content(&self, product_id: &str) -> Result<Vec<ProductContent>> {
        self.pool
            .with_conn(|conn| get_all_content(conn, product_id))
    }

    /// Get cover image for a product
    pub fn get_cover(&self, product_id: &str) -> Result<Option<ProductContent>> {
        self.pool.with_conn(|conn| get_cover(conn, product_id))
    }

    /// Get all screenshots for a product
    pub fn get_screenshots(&self, product_id: &str) -> Result<Vec<ProductContent>> {
        self.pool
            .with_conn(|conn| get_screenshots(conn, product_id))
    }

    /// Save product content (upsert)
    pub fn save_content(&self, content: &ProductContent) -> Result<i64> {
        self.pool
            .with_conn(|conn| save_product_content(conn, content))
    }

    /// Delete all content for a product
    pub fn delete_content(&self, product_id: &str) -> Result<()> {
        self.pool
            .with_conn(|conn| delete_product_content(conn, product_id))
    }

    /// Delete a product and all its content
    pub fn delete_product(&self, product_id: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            delete_product_content(conn, product_id)?;
            delete_metadata(conn, product_id)?;
            Ok(())
        })
    }
}

impl std::fmt::Debug for LibraryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryService")
            .field("pool", &self.pool)
            .finish()
    }
}
