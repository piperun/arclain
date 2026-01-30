//! Content reference storage operations
//!
//! Handles storing and retrieving content references (images, thumbnails, etc.)
//! linked to product metadata.

use gameta_core::{ContentReference, ContentType, StorageError};
use libsql::params;

use super::LibSqlBackend;

impl LibSqlBackend {
    /// Save a content reference
    ///
    /// Links a cached content item (image, thumbnail, etc.) to a product.
    pub async fn save_content(&self, content: &ContentReference) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT INTO product_content
                 (product_id, content_type, content_index, cache_key, source_url, width, height)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    content.product_id.clone(),
                    content.content_type.as_str().to_string(),
                    content.index,
                    content.cache_key.clone(),
                    content.source_url.clone(),
                    content.width,
                    content.height,
                ],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// Get all content references for a product
    ///
    /// # Arguments
    /// * `product_id` - The product's composite ID
    pub async fn get_content(&self, product_id: &str) -> Result<Vec<ContentReference>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT product_id, content_type, content_index, cache_key, source_url, width, height
                 FROM product_content WHERE product_id = ?1",
                params![product_id],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        let mut content = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            content.push(ContentReference {
                product_id: row.get(0).unwrap_or_default(),
                content_type: ContentType::from_str(&row.get::<String>(1).unwrap_or_default())
                    .unwrap_or(ContentType::Other),
                index: row.get(2).unwrap_or(0),
                cache_key: row.get(3).unwrap_or_default(),
                source_url: row.get(4).ok(),
                width: row.get(5).ok(),
                height: row.get(6).ok(),
            });
        }
        Ok(content)
    }

    /// Delete all content references for a product
    pub async fn delete_content(&self, product_id: &str) -> Result<(), StorageError> {
        self.conn
            .execute(
                "DELETE FROM product_content WHERE product_id = ?1",
                params![product_id],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }
}
