//! SQLite implementation of StorageBackend

use anyhow::Result;
use metastore_abstract::{StorageBackend, StorageError};
use metastore_types::{ContentReference, MetadataSource, ProductMetadata};
use mini_orm::SqliteDb;
use std::sync::Arc;

/// SQLite-backed storage for metastore
pub struct SqliteBackend {
    db: Arc<SqliteDb>,
}

impl SqliteBackend {
    /// Create a new SQLite backend with the given database connection
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    /// Initialize the database schema
    pub fn init_schema(&self) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS product_metadata (
                    id TEXT PRIMARY KEY,
                    source TEXT NOT NULL,
                    external_id TEXT NOT NULL,
                    title TEXT,
                    creator TEXT,
                    description TEXT,
                    release_date TEXT,
                    price INTEGER,
                    currency TEXT,
                    rating REAL,
                    rating_count INTEGER,
                    purchase_count INTEGER,
                    favorite_count INTEGER,
                    review_count INTEGER,
                    file_size TEXT,
                    file_format TEXT,
                    age_rating TEXT,
                    genres TEXT,
                    tags TEXT,
                    languages TEXT,
                    extras TEXT,
                    raw_api_response TEXT,
                    raw_html TEXT,
                    cached_at INTEGER NOT NULL,
                    updated_at INTEGER
                )",
                [],
            )?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_product_metadata_external 
                 ON product_metadata(source, external_id)",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS product_content (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    product_id TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    content_index INTEGER NOT NULL,
                    cache_key TEXT NOT NULL,
                    source_url TEXT,
                    width INTEGER,
                    height INTEGER,
                    FOREIGN KEY(product_id) REFERENCES product_metadata(id)
                )",
                [],
            )?;

            Ok(())
        })
    }
}

impl StorageBackend for SqliteBackend {
    fn save_metadata(&self, meta: &ProductMetadata) -> Result<(), StorageError> {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO product_metadata 
                     (id, source, external_id, title, creator, description, release_date,
                      price, currency, rating, rating_count, purchase_count, favorite_count,
                      review_count, file_size, file_format, age_rating, genres, tags, languages,
                      extras, raw_api_response, raw_html, cached_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                             ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                    rusqlite::params![
                        meta.id,
                        meta.source.as_str(),
                        meta.external_id,
                        meta.title,
                        meta.creator,
                        meta.description,
                        meta.release_date,
                        meta.price,
                        meta.currency,
                        meta.rating,
                        meta.rating_count,
                        meta.purchase_count,
                        meta.favorite_count,
                        meta.review_count,
                        meta.file_size,
                        meta.file_format,
                        meta.age_rating,
                        serde_json::to_string(&meta.genres).ok(),
                        serde_json::to_string(&meta.tags).ok(),
                        serde_json::to_string(&meta.languages).ok(),
                        meta.extras.to_string(),
                        meta.raw_api_response,
                        meta.raw_html,
                        meta.cached_at,
                        meta.updated_at,
                    ],
                )?;
                Ok(())
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>, StorageError> {
        self.db
            .with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, source, external_id, title, creator, description, release_date,
                            price, currency, rating, rating_count, purchase_count, favorite_count,
                            review_count, file_size, file_format, age_rating, genres, tags, languages,
                            extras, raw_api_response, raw_html, cached_at, updated_at
                     FROM product_metadata WHERE id = ?1",
                )?;

                let result = stmt.query_row([id], |row| {
                    Ok(row_to_metadata(row))
                });

                match result {
                    Ok(meta) => Ok(Some(meta)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Query failed: {}", e)),
                }
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>, StorageError> {
        self.db
            .with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, source, external_id, title, creator, description, release_date,
                            price, currency, rating, rating_count, purchase_count, favorite_count,
                            review_count, file_size, file_format, age_rating, genres, tags, languages,
                            extras, raw_api_response, raw_html, cached_at, updated_at
                     FROM product_metadata WHERE source = ?1 AND external_id = ?2",
                )?;

                let result = stmt.query_row([source.as_str(), external_id], |row| {
                    Ok(row_to_metadata(row))
                });

                match result {
                    Ok(meta) => Ok(Some(meta)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(anyhow::anyhow!("Query failed: {}", e)),
                }
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn delete_metadata(&self, id: &str) -> Result<(), StorageError> {
        self.db
            .with_connection(|conn| {
                conn.execute("DELETE FROM product_metadata WHERE id = ?1", [id])?;
                Ok(())
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn list_all(&self) -> Result<Vec<String>, StorageError> {
        self.db
            .with_connection(|conn| {
                let mut stmt = conn.prepare("SELECT id FROM product_metadata")?;
                let ids: Vec<String> = stmt
                    .query_map([], |row| row.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                Ok(ids)
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn save_content(&self, content: &ContentReference) -> Result<(), StorageError> {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO product_content 
                     (product_id, content_type, content_index, cache_key, source_url, width, height)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        content.product_id,
                        content.content_type.as_str(),
                        content.index,
                        content.cache_key,
                        content.source_url,
                        content.width,
                        content.height,
                    ],
                )?;
                Ok(())
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }

    fn get_content(&self, product_id: &str) -> Result<Vec<ContentReference>, StorageError> {
        self.db
            .with_connection(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT product_id, content_type, content_index, cache_key, source_url, width, height
                     FROM product_content WHERE product_id = ?1",
                )?;

                let content: Vec<ContentReference> = stmt
                    .query_map([product_id], |row| {
                        Ok(ContentReference {
                            product_id: row.get(0)?,
                            content_type: match row.get::<_, String>(1)?.as_str() {
                                "cover" => metastore_types::ContentType::Cover,
                                "screenshot" => metastore_types::ContentType::Screenshot,
                                "thumbnail" => metastore_types::ContentType::Thumbnail,
                                "banner" => metastore_types::ContentType::Banner,
                                "video" => metastore_types::ContentType::Video,
                                _ => metastore_types::ContentType::Other,
                            },
                            index: row.get(2)?,
                            cache_key: row.get(3)?,
                            source_url: row.get(4)?,
                            width: row.get(5)?,
                            height: row.get(6)?,
                        })
                    })?
                    .filter_map(|r| r.ok())
                    .collect();

                Ok(content)
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))
    }
}

/// Helper to convert a row to ProductMetadata
fn row_to_metadata(row: &rusqlite::Row) -> ProductMetadata {
    ProductMetadata {
        id: row.get(0).unwrap_or_default(),
        source: MetadataSource::from_str(&row.get::<_, String>(1).unwrap_or_default())
            .unwrap_or(MetadataSource::Custom),
        external_id: row.get(2).unwrap_or_default(),
        title: row.get(3).ok(),
        creator: row.get(4).ok(),
        description: row.get(5).ok(),
        release_date: row.get(6).ok(),
        price: row.get(7).ok(),
        currency: row.get(8).ok(),
        rating: row.get(9).ok(),
        rating_count: row.get(10).ok(),
        purchase_count: row.get(11).ok(),
        favorite_count: row.get(12).ok(),
        review_count: row.get(13).ok(),
        file_size: row.get(14).ok(),
        file_format: row.get(15).ok(),
        age_rating: row.get(16).ok(),
        genres: row
            .get::<_, Option<String>>(17)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        tags: row
            .get::<_, Option<String>>(18)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        languages: row
            .get::<_, Option<String>>(19)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        extras: row
            .get::<_, Option<String>>(20)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
        raw_api_response: row.get(21).ok(),
        raw_html: row.get(22).ok(),
        geo_blocked: false, // TODO: Add column and read from DB
        cached_at: row.get(23).unwrap_or(0),
        updated_at: row.get(24).ok(),
    }
}
