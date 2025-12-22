//! Database abstraction layer

use metastore_types::{ContentReference, ProductMetadata};

/// Trait for storage backends
pub trait StorageBackend: Send + Sync {
    /// Save product metadata
    fn save_metadata(&self, meta: &ProductMetadata) -> Result<(), StorageError>;

    /// Get metadata by ID (e.g., "dlsite:RJ123456")
    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>, StorageError>;

    /// Get metadata by external ID and source
    fn get_by_external_id(
        &self,
        source: metastore_types::MetadataSource,
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
}

#[derive(Debug)]
pub enum StorageError {
    NotFound,
    ConnectionFailed(String),
    QueryFailed(String),
    SerializationError(String),
    Other(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Not found"),
            Self::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            Self::QueryFailed(msg) => write!(f, "Query failed: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}
impl std::error::Error for StorageError {}
