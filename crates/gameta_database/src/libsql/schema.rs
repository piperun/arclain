//! Database schema initialization
//!
//! Creates all required tables and indexes for the gameta database.

use anyhow::Result;

use super::LibSqlBackend;

impl LibSqlBackend {
    /// Initialize the database schema
    ///
    /// Creates all tables and indexes if they don't exist:
    /// - `product_metadata` - Main metadata storage
    /// - `product_content` - Content references (images, etc.)
    /// - `content_refs` - Content integrity tracking (SRI hashes)
    /// - `fetch_log` - Request logging for rate limiting
    pub async fn init_schema(&self) -> Result<()> {
        self.create_metadata_table().await?;
        self.create_content_table().await?;
        self.create_integrity_table().await?;
        self.create_fetch_log_table().await?;
        Ok(())
    }

    async fn create_metadata_table(&self) -> Result<()> {
        self.conn
            .execute(
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
                    geo_blocked INTEGER DEFAULT 0,
                    cached_at INTEGER NOT NULL,
                    updated_at INTEGER
                )",
                (),
            )
            .await?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_product_metadata_external
                 ON product_metadata(source, external_id)",
                (),
            )
            .await?;

        Ok(())
    }

    async fn create_content_table(&self) -> Result<()> {
        self.conn
            .execute(
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
                (),
            )
            .await?;

        Ok(())
    }

    async fn create_integrity_table(&self) -> Result<()> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS content_refs (
                    id INTEGER PRIMARY KEY,
                    product_id TEXT NOT NULL,
                    content_type TEXT NOT NULL,
                    cache_key TEXT NOT NULL UNIQUE,
                    sri_hash TEXT NOT NULL,
                    source_url TEXT,
                    size_bytes INTEGER,
                    fetched_at TEXT NOT NULL,
                    verified_at TEXT,
                    FOREIGN KEY (product_id) REFERENCES product_metadata(id)
                )",
                (),
            )
            .await?;

        Ok(())
    }

    async fn create_fetch_log_table(&self) -> Result<()> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS fetch_log (
                    id INTEGER PRIMARY KEY,
                    source TEXT NOT NULL,
                    endpoint TEXT NOT NULL,
                    product_id TEXT,
                    requested_at TEXT NOT NULL,
                    response_status INTEGER,
                    response_size INTEGER,
                    cached INTEGER DEFAULT 0
                )",
                (),
            )
            .await?;

        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_fetch_log_source_time
                 ON fetch_log(source, requested_at)",
                (),
            )
            .await?;

        Ok(())
    }
}
