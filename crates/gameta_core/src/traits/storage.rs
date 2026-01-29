//! Storage abstraction layer

use crate::errors::StorageError;
use crate::types::{ContentReference, MetadataSource, ProductMetadata};

/// Trait for storage backends
///
/// Implementations can be SQLite, PostgreSQL, in-memory, etc.
pub trait StorageBackend: Send + Sync {
    /// Save product metadata
    fn save_metadata(&self, meta: &ProductMetadata) -> Result<(), StorageError>;

    /// Get metadata by ID (e.g., "dlsite:RJ123456")
    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>, StorageError>;

    /// Get metadata by external ID and source
    fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>, StorageError>;

    /// Delete metadata
    fn delete_metadata(&self, id: &str) -> Result<(), StorageError>;

    /// List all metadata IDs
    fn list_all(&self) -> Result<Vec<String>, StorageError>;

    /// Save content reference
    fn save_content(&self, content: &ContentReference) -> Result<(), StorageError>;

    /// Get content references for a product
    fn get_content(&self, product_id: &str) -> Result<Vec<ContentReference>, StorageError>;

    /// Delete content references for a product
    fn delete_content(&self, product_id: &str) -> Result<(), StorageError>;
}
