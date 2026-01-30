//! Product metadata CRUD operations

use gameta_core::{MetadataSource, ProductMetadata, StorageError};
use libsql::params;

use super::helpers::row_to_metadata;
use super::LibSqlBackend;

/// SQL for selecting all metadata columns
const SELECT_METADATA: &str = "SELECT id, source, external_id, title, creator, description, release_date,
        price, currency, rating, rating_count, purchase_count, favorite_count,
        review_count, file_size, file_format, age_rating, genres, tags, languages,
        extras, raw_api_response, raw_html, geo_blocked, cached_at, updated_at
 FROM product_metadata";

impl LibSqlBackend {
    /// Save product metadata
    ///
    /// Uses INSERT OR REPLACE to upsert the record.
    pub async fn save_metadata(&self, meta: &ProductMetadata) -> Result<(), StorageError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO product_metadata
                 (id, source, external_id, title, creator, description, release_date,
                  price, currency, rating, rating_count, purchase_count, favorite_count,
                  review_count, file_size, file_format, age_rating, genres, tags, languages,
                  extras, raw_api_response, raw_html, geo_blocked, cached_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                params![
                    meta.id.clone(),
                    meta.source.as_str().to_string(),
                    meta.external_id.clone(),
                    meta.title.clone(),
                    meta.creator.clone(),
                    meta.description.clone(),
                    meta.release_date.clone(),
                    meta.price,
                    meta.currency.clone(),
                    meta.rating,
                    meta.rating_count,
                    meta.purchase_count,
                    meta.favorite_count,
                    meta.review_count,
                    meta.file_size.clone(),
                    meta.file_format.clone(),
                    meta.age_rating.clone(),
                    serde_json::to_string(&meta.genres).ok(),
                    serde_json::to_string(&meta.tags).ok(),
                    serde_json::to_string(&meta.languages).ok(),
                    meta.extras.to_string(),
                    meta.raw_api_response.clone(),
                    meta.raw_html.clone(),
                    meta.geo_blocked as i32,
                    meta.cached_at,
                    meta.updated_at,
                ],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// Get metadata by ID
    ///
    /// # Arguments
    /// * `id` - The composite ID (e.g., "dlsite:RJ123456")
    pub async fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>, StorageError> {
        let query = format!("{} WHERE id = ?1", SELECT_METADATA);
        let mut rows = self
            .conn
            .query(&query, params![id])
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            Ok(Some(row_to_metadata(&row)?))
        } else {
            Ok(None)
        }
    }

    /// Get metadata by external ID and source
    ///
    /// # Arguments
    /// * `source` - The metadata source (DLSite, Steam, etc.)
    /// * `external_id` - The source-specific ID (e.g., "RJ123456")
    pub async fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>, StorageError> {
        let query = format!("{} WHERE source = ?1 AND external_id = ?2", SELECT_METADATA);
        let mut rows = self
            .conn
            .query(&query, params![source.as_str(), external_id])
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            Ok(Some(row_to_metadata(&row)?))
        } else {
            Ok(None)
        }
    }

    /// Delete metadata by ID
    pub async fn delete_metadata(&self, id: &str) -> Result<(), StorageError> {
        self.conn
            .execute("DELETE FROM product_metadata WHERE id = ?1", params![id])
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// List all metadata IDs
    pub async fn list_all(&self) -> Result<Vec<String>, StorageError> {
        let mut rows = self
            .conn
            .query("SELECT id FROM product_metadata", ())
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        let mut ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            if let Ok(id) = row.get::<String>(0) {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}
