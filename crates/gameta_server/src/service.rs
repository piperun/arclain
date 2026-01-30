//! Metadata service - core business logic
//!
//! Handles:
//! - Fetching metadata from providers
//! - Caching responses
//! - Database storage
//! - Background workers

use gameta_core::{MetadataProvider, MetadataSource, ProductMetadata, SearchResult};
use gameta_database::LibSqlBackend;
use gameta_lib::providers::dlsite::DLSiteProvider;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ServerConfig;

/// The metadata service
pub struct MetadataService {
    config: ServerConfig,
    db: Arc<RwLock<Option<LibSqlBackend>>>,
    cache_dir: PathBuf,
}

impl MetadataService {
    /// Create a new metadata service
    pub fn new(config: ServerConfig) -> Self {
        let cache_dir = config.cache_dir.clone();
        Self {
            config,
            db: Arc::new(RwLock::new(None)),
            cache_dir,
        }
    }

    /// Initialize the database connection
    pub async fn init(&self) -> anyhow::Result<()> {
        let db = LibSqlBackend::new_local(&self.config.database_path).await?;
        db.init_schema().await?;

        let mut db_lock = self.db.write().await;
        *db_lock = Some(db);

        tracing::info!("Database initialized at {:?}", self.config.database_path);
        Ok(())
    }

    /// Get metadata for a product (from database cache)
    pub async fn get_metadata(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> anyhow::Result<Option<ProductMetadata>> {
        let db_lock = self.db.read().await;
        let db = db_lock.as_ref().ok_or_else(|| anyhow::anyhow!("Database not initialized"))?;

        let id = format!("{}:{}", source.as_str(), external_id);
        match db.get_metadata(&id).await {
            Ok(meta) => Ok(meta),
            Err(e) => {
                tracing::warn!("Failed to get metadata: {}", e);
                Ok(None)
            }
        }
    }

    /// Fetch metadata from source (network) and cache it
    pub async fn fetch_metadata(
        &self,
        source: MetadataSource,
        external_id: &str,
        force: bool,
    ) -> anyhow::Result<ProductMetadata> {
        // Check cache first (unless force refresh)
        if !force {
            if let Some(cached) = self.get_metadata(source.clone(), external_id).await? {
                tracing::debug!("Returning cached metadata for {}:{}", source.as_str(), external_id);
                return Ok(cached);
            }
        }

        // Log the fetch attempt
        {
            let db_lock = self.db.read().await;
            if let Some(db) = db_lock.as_ref() {
                let _ = db.log_fetch(source.as_str(), "metadata", Some(external_id), None, None, false).await;
            }
        }

        // Get provider and build requests
        let metadata = match source {
            MetadataSource::DLSite => {
                self.fetch_dlsite_metadata(external_id).await?
            }
            _ => {
                anyhow::bail!("Source {:?} not yet supported", source);
            }
        };

        // Save to database
        {
            let db_lock = self.db.read().await;
            if let Some(db) = db_lock.as_ref() {
                db.save_metadata(&metadata).await?;
                tracing::info!("Saved metadata for {}", metadata.id);
            }
        }

        Ok(metadata)
    }

    /// Fetch DLSite metadata specifically
    async fn fetch_dlsite_metadata(&self, external_id: &str) -> anyhow::Result<ProductMetadata> {
        use gameta_core::HttpResponse;

        let provider = DLSiteProvider::new();
        let requests = provider.request_metadata(external_id);

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

        let mut responses: Vec<(&str, HttpResponse)> = Vec::new();

        for req in &requests {
            tracing::debug!("Fetching: {}", req.url);

            let mut request_builder = match req.method {
                gameta_core::HttpMethod::Get => client.get(&req.url),
                gameta_core::HttpMethod::Post => client.post(&req.url),
            };

            // Add headers
            for (key, value) in &req.headers {
                request_builder = request_builder.header(key.as_str(), value.as_str());
            }

            let response = request_builder.send().await?;
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let body = response.bytes().await?.to_vec();

            let http_response = HttpResponse {
                status,
                body,
                content_type,
            };

            // Store response with key
            responses.push((req.cache_key.as_str(), http_response));

            // Log fetch
            {
                let db_lock = self.db.read().await;
                if let Some(db) = db_lock.as_ref() {
                    let _ = db.log_fetch(
                        "dlsite",
                        if req.cache_key.contains(":ajax:") { "api" } else { "html" },
                        Some(external_id),
                        Some(status as i32),
                        Some(responses.last().map(|(_, r)| r.body.len() as i64).unwrap_or(0)),
                        false,
                    ).await;
                }
            }
        }

        // Parse responses
        let refs: Vec<(&str, HttpResponse)> = responses.iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();

        provider.parse_responses(external_id, &refs)
            .map_err(|e| anyhow::anyhow!("Failed to parse metadata: {:?}", e))
    }

    /// Search for products
    pub async fn search(
        &self,
        query: &str,
        source: Option<MetadataSource>,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let source = source.unwrap_or(MetadataSource::DLSite);

        match source {
            MetadataSource::DLSite => self.search_dlsite(query).await,
            _ => {
                tracing::warn!("Search not implemented for {:?}", source);
                Ok(vec![])
            }
        }
    }

    /// Search DLSite
    async fn search_dlsite(&self, query: &str) -> anyhow::Result<Vec<SearchResult>> {
        use gameta_core::HttpResponse;

        let provider = DLSiteProvider::new();
        let request = provider.request_search(query);

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

        let mut request_builder = client.get(&request.url);
        for (key, value) in &request.headers {
            request_builder = request_builder.header(key.as_str(), value.as_str());
        }

        // Log the search
        {
            let db_lock = self.db.read().await;
            if let Some(db) = db_lock.as_ref() {
                let _ = db.log_fetch("dlsite", "search", None, None, None, false).await;
            }
        }

        let response = request_builder.send().await?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = response.bytes().await?.to_vec();

        let http_response = HttpResponse {
            status,
            body,
            content_type,
        };

        provider.parse_search(&http_response)
            .map_err(|e| anyhow::anyhow!("Failed to parse search: {:?}", e))
    }

    /// Get content from cache
    pub async fn get_content(&self, cache_key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match cacache::read(&self.cache_dir, cache_key).await {
            Ok(data) => Ok(Some(data)),
            Err(cacache::Error::EntryNotFound(_, _)) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("Cache read error: {}", e)),
        }
    }

    /// Save content to cache with integrity tracking
    pub async fn save_content(
        &self,
        cache_key: &str,
        product_id: &str,
        content_type: &str,
        data: &[u8],
        source_url: Option<&str>,
    ) -> anyhow::Result<String> {
        // Write to cacache (returns SRI hash)
        let sri = cacache::write(&self.cache_dir, cache_key, data).await?;
        let sri_string = sri.to_string();

        // Track in database
        {
            let db_lock = self.db.read().await;
            if let Some(db) = db_lock.as_ref() {
                db.save_content_ref(
                    product_id,
                    content_type,
                    cache_key,
                    &sri_string,
                    source_url,
                    Some(data.len() as i64),
                ).await?;
            }
        }

        Ok(sri_string)
    }

    /// Verify content integrity
    pub async fn verify_content(&self, cache_key: &str) -> anyhow::Result<bool> {
        let db_lock = self.db.read().await;
        let db = db_lock.as_ref().ok_or_else(|| anyhow::anyhow!("Database not initialized"))?;

        // Get expected hash from database
        let expected_hash = match db.get_content_hash(cache_key).await? {
            Some(hash) => hash,
            None => return Ok(false), // No record of this content
        };

        // Read from cache and verify
        match cacache::read(&self.cache_dir, cache_key).await {
            Ok(data) => {
                // Compute actual hash
                let actual_sri = cacache::write(&self.cache_dir, cache_key, &data).await?;
                let matches = actual_sri.to_string() == expected_hash;

                if matches {
                    // Mark as verified
                    db.mark_verified(cache_key).await?;
                }

                Ok(matches)
            }
            Err(_) => Ok(false),
        }
    }

    /// Get recent fetch count (for rate limiting)
    pub async fn recent_fetch_count(&self, source: &str, since_seconds: u64) -> anyhow::Result<i64> {
        let db_lock = self.db.read().await;
        let db = db_lock.as_ref().ok_or_else(|| anyhow::anyhow!("Database not initialized"))?;

        // Simple timestamp calculation (production should use proper datetime)
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let since = (now - since_seconds).to_string();

        db.count_recent_fetches(source, &since).await
            .map_err(|e| anyhow::anyhow!("Failed to count fetches: {:?}", e))
    }

    /// Get service configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }
}

/// Create a shared service instance
pub fn create_service(config: ServerConfig) -> Arc<MetadataService> {
    Arc::new(MetadataService::new(config))
}
