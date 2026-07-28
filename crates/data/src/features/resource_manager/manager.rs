//! Resource Manager - orchestrates data fetching and storage
//!
//! Provides a unified interface for getting data from network, disk, or cache
//! without exposing storage details to consumers.

use crate::features::content_cache::{CacheOwner, ContentCache};
use crate::shared::{
    read_to_end_with_limit, safe_log_fingerprint, ResourceConfig, ResourceData, ResourceRequest,
    ResourceSource, ResourceType, StorageStrategy, DEFAULT_MAX_RESOURCE_SIZE_BYTES,
};
use arclain_db::CacheType;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Minimum valid image size (bytes) - images smaller than this are likely corrupted/placeholders
const MIN_IMAGE_SIZE: usize = 100;

/// Result of image validation
#[derive(Debug)]
pub enum ImageValidation {
    Valid,
    TooSmall(usize),
    InvalidFormat,
    NotAnImage,
}

/// Validate that data is a valid image by checking magic bytes and size
pub fn validate_image(data: &[u8]) -> ImageValidation {
    if data.len() < MIN_IMAGE_SIZE {
        return ImageValidation::TooSmall(data.len());
    }

    // Check magic bytes for common image formats
    let is_jpeg = data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF;
    let is_png = data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n";
    let is_webp = data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP";
    let is_gif = data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a");
    let is_bmp = data.len() >= 2 && &data[0..2] == b"BM";

    if is_jpeg || is_png || is_webp || is_gif || is_bmp {
        ImageValidation::Valid
    } else {
        // Check if it looks like HTML (common error response)
        if data.len() > 15
            && (data.starts_with(b"<!DOCTYPE")
                || data.starts_with(b"<html")
                || data.starts_with(b"<HTML"))
        {
            ImageValidation::NotAnImage
        } else {
            ImageValidation::InvalidFormat
        }
    }
}

/// Manages resource fetching, caching, and retrieval
pub struct ResourceManager {
    /// Content cache (optional - can be disabled)
    cache: Option<Arc<ContentCache>>,
    /// Fallback disk directory
    fallback_dir: Option<PathBuf>,
    /// In-memory store for ephemeral resources
    memory_store: RwLock<HashMap<String, Vec<u8>>>,
    /// Configuration
    config: ResourceConfig,
}

impl ResourceManager {
    /// Create a new ResourceManager with cache
    pub fn new(cache: Arc<ContentCache>, config: ResourceConfig) -> Self {
        Self {
            cache: Some(cache),
            fallback_dir: config.fallback_dir.clone(),
            memory_store: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Create a ResourceManager without cache (disk/memory only)
    pub fn without_cache(config: ResourceConfig) -> Self {
        Self {
            cache: None,
            fallback_dir: config.fallback_dir.clone(),
            memory_store: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Check if a resource exists (in any storage)
    pub fn has(&self, key: &str) -> bool {
        self.has_for_owner(&CacheOwner::host(), key)
    }

    pub fn has_for_owner(&self, owner: &CacheOwner, key: &str) -> bool {
        let scoped_key = owner.scoped_key(key);
        // Check memory first
        let memory = self.memory_store.read();
        if memory.contains_key(&scoped_key)
            || (matches!(owner, CacheOwner::Host) && memory.contains_key(key))
        {
            return true;
        }
        drop(memory);

        // Check cache
        if let Some(cache) = &self.cache {
            if cache.has_for_owner(owner, key).unwrap_or(false) {
                return true;
            }
        }

        // Check disk fallback
        if let Some(dir) = &self.fallback_dir {
            let scoped_path = dir.join(sanitize_key(&scoped_key));
            if scoped_path.exists()
                || (matches!(owner, CacheOwner::Host) && dir.join(sanitize_key(key)).exists())
            {
                return true;
            }
        }

        false
    }

    /// Get a resource synchronously (from storage only, no fetch)
    pub fn get(&self, key: &str) -> Option<ResourceData> {
        let limit = self.materialization_limit();
        self.get_with_limit(key, limit)
    }

    /// Get a resource without materializing more than `limit` bytes.
    pub fn get_with_limit(&self, key: &str, limit: usize) -> Option<ResourceData> {
        self.get_with_limit_for_owner(&CacheOwner::host(), key, limit)
    }

    pub fn get_with_limit_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        limit: usize,
    ) -> Option<ResourceData> {
        let limit = limit.min(self.materialization_limit());
        let scoped_key = owner.scoped_key(key);
        // Try memory first
        let memory = self.memory_store.read();
        let memory_data = memory.get(&scoped_key).or_else(|| {
            matches!(owner, CacheOwner::Host)
                .then(|| memory.get(key))
                .flatten()
        });
        if let Some(data) = memory_data {
            if data.len() <= limit {
                return Some(ResourceData {
                    data: data.clone(),
                    content_type: None,
                    source: ResourceSource::Memory,
                });
            }
            warn!(
                "[ResourceManager] Memory entry '{}' exceeds the {}-byte materialized read limit",
                safe_log_fingerprint(key),
                limit
            );
        }
        drop(memory);

        // Try cache
        if let Some(cache) = &self.cache {
            match cache.get_with_limit_for_owner(owner, key, limit) {
                Ok(Some(data)) => {
                    return Some(ResourceData {
                        data,
                        content_type: None,
                        source: ResourceSource::Cache,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(
                        "[ResourceManager] Bounded cache read failed: {}",
                        safe_log_fingerprint(error.to_string())
                    );
                }
            }
        }

        // Try disk fallback
        if let Some(dir) = &self.fallback_dir {
            let scoped_path = dir.join(sanitize_key(&scoped_key));
            let path = if scoped_path.exists() || !matches!(owner, CacheOwner::Host) {
                scoped_path
            } else {
                dir.join(sanitize_key(key))
            };
            let declared_oversize = path.metadata().ok().is_some_and(|metadata| {
                usize::try_from(metadata.len()).map_or(true, |length| length > limit)
            });
            if declared_oversize {
                warn!(
                    "[ResourceManager] Fallback entry '{}' exceeds the {}-byte materialized read limit",
                    safe_log_fingerprint(key),
                    limit
                );
            } else if let Ok(mut file) = File::open(&path) {
                match read_to_end_with_limit(&mut file, limit, "fallback resource") {
                    Ok(data) => {
                        return Some(ResourceData {
                            data,
                            content_type: None,
                            source: ResourceSource::Disk,
                        });
                    }
                    Err(error) => {
                        warn!(
                            "[ResourceManager] Bounded fallback read failed: {}",
                            safe_log_fingerprint(error.to_string())
                        );
                    }
                }
            }
        }

        None
    }

    /// Store a resource directly (without fetching)
    pub fn put(&self, key: &str, data: &[u8], request: &ResourceRequest) -> Result<(), String> {
        self.put_for_owner(&CacheOwner::host(), key, data, request)
    }

    pub fn put_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        data: &[u8],
        request: &ResourceRequest,
    ) -> Result<(), String> {
        let strategy = request
            .storage_override
            .unwrap_or(self.config.default_strategy);

        // Check size limit
        if let Some(max) = self.config.max_resource_size {
            if data.len() > max {
                return Err(format!(
                    "Resource too large: {} bytes (max: {})",
                    data.len(),
                    max
                ));
            }
        }

        // Validate images before storing
        if request.resource_type == ResourceType::Image {
            match validate_image(data) {
                ImageValidation::Valid => {}
                ImageValidation::TooSmall(size) => {
                    warn!(
                        "[ResourceManager] Rejecting image '{}': too small ({} bytes, min: {})",
                        safe_log_fingerprint(key),
                        size,
                        MIN_IMAGE_SIZE
                    );
                    return Err(format!("Image too small: {} bytes", size));
                }
                ImageValidation::InvalidFormat => {
                    warn!(
                        "[ResourceManager] Rejecting image '{}': invalid format (unknown magic bytes)",
                        safe_log_fingerprint(key)
                    );
                    return Err("Invalid image format".to_string());
                }
                ImageValidation::NotAnImage => {
                    warn!(
                        "[ResourceManager] Rejecting image '{}': not an image (looks like HTML/text)",
                        safe_log_fingerprint(key)
                    );
                    return Err("Data is not an image (received HTML/text instead)".to_string());
                }
            }
        }

        match strategy {
            StorageStrategy::Cache => {
                if let Some(cache) = &self.cache {
                    // Infer cache type from key for more accurate categorization
                    // Fall back to resource_type hint if key doesn't match patterns
                    let cache_type = CacheType::from_key(key);
                    let cache_type = if cache_type == CacheType::Other {
                        // Key didn't match known patterns, use resource_type hint
                        match request.resource_type {
                            ResourceType::Image => CacheType::Screenshot,
                            ResourceType::Metadata => CacheType::Metadata,
                            _ => CacheType::Other,
                        }
                    } else {
                        cache_type
                    };

                    // Extract product_id from key if not provided
                    let product_id = request
                        .product_id
                        .clone()
                        .or_else(|| CacheType::extract_product_id(key));

                    cache
                        .put_for_owner(
                            owner,
                            key,
                            data,
                            cache_type,
                            product_id.as_deref(),
                            request.url.as_deref(), // source_url
                        )
                        .map_err(|e| e.to_string())?;
                    debug!(
                        "Stored {} bytes to cache: {}",
                        data.len(),
                        safe_log_fingerprint(key)
                    );
                } else {
                    // Fallback to disk if cache disabled
                    self.store_to_disk_for_owner(owner, key, data)?;
                }
            }
            StorageStrategy::Disk => {
                self.store_to_disk_for_owner(owner, key, data)?;
            }
            StorageStrategy::Memory => {
                self.memory_store
                    .write()
                    .insert(owner.scoped_key(key), data.to_vec());
                debug!(
                    "Stored {} bytes to memory: {}",
                    data.len(),
                    safe_log_fingerprint(key)
                );
            }
            StorageStrategy::NoStore => {
                debug!(
                    "NoStore strategy - not storing: {}",
                    safe_log_fingerprint(key)
                );
            }
        }

        Ok(())
    }

    fn store_to_disk_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        data: &[u8],
    ) -> Result<(), String> {
        let dir = self
            .fallback_dir
            .as_ref()
            .ok_or_else(|| "No fallback directory configured".to_string())?;

        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

        let path = dir.join(sanitize_key(&owner.scoped_key(key)));
        std::fs::write(&path, data).map_err(|e| e.to_string())?;

        debug!(
            "Stored {} bytes to disk: {}",
            data.len(),
            safe_log_fingerprint(path.as_os_str().to_string_lossy().as_bytes())
        );
        Ok(())
    }

    /// Fetch and store a resource (blocking)
    ///
    /// Note: For async fetching, use request_async + poll_status pattern
    pub fn fetch_sync<R: std::io::Read>(
        &self,
        request: &ResourceRequest,
        http_get: impl FnOnce(&str) -> Result<R, String>,
    ) -> Result<ResourceData, String> {
        let key = &request.key;

        // Check if already cached
        if let Some(data) = self.get(key) {
            info!("Resource found in storage: {}", safe_log_fingerprint(key));
            return Ok(data);
        }

        // Fetch from source
        let limit = self.materialization_limit();
        let (data, source) = if let Some(url) = &request.url {
            info!(
                "Fetching resource from network: {}",
                safe_log_fingerprint(url)
            );
            let mut reader = http_get(url)?;
            (
                read_to_end_with_limit(&mut reader, limit, "fetched resource")
                    .map_err(|error| error.to_string())?,
                ResourceSource::Network,
            )
        } else if let Some(path) = &request.path {
            info!(
                "Reading resource from disk: {}",
                safe_log_fingerprint(path.as_os_str().to_string_lossy().as_bytes())
            );
            let mut file = File::open(path).map_err(|error| error.to_string())?;
            (
                read_to_end_with_limit(&mut file, limit, "local resource")
                    .map_err(|error| error.to_string())?,
                ResourceSource::Disk,
            )
        } else {
            return Err("No URL or path provided for resource".to_string());
        };

        // Store it
        self.put(key, &data, request)?;

        Ok(ResourceData {
            data,
            content_type: None,
            source,
        })
    }

    /// Delete a resource from disk storage only
    /// Note: ContentCache doesn't expose a delete method currently
    pub fn delete_from_disk(&self, key: &str) -> Result<(), String> {
        // Remove from memory
        self.memory_store.write().remove(key);

        // Remove from disk
        if let Some(dir) = &self.fallback_dir {
            let path = dir.join(sanitize_key(key));
            let _ = std::fs::remove_file(path);
        }

        Ok(())
    }

    /// Clear all memory-stored resources
    pub fn clear_memory(&self) {
        self.memory_store.write().clear();
    }

    /// Get current config
    pub fn config(&self) -> &ResourceConfig {
        &self.config
    }

    /// Effective ceiling for APIs that return a fully materialized body.
    pub fn materialization_limit(&self) -> usize {
        self.config
            .max_resource_size
            .unwrap_or(DEFAULT_MAX_RESOURCE_SIZE_BYTES)
    }

    /// Update config
    pub fn set_config(&mut self, config: ResourceConfig) {
        self.fallback_dir = config.fallback_dir.clone();
        self.config = config;
    }

    /// Check if caching is enabled
    pub fn is_cache_enabled(&self) -> bool {
        self.cache.is_some() && self.config.caching_enabled
    }
}

/// Sanitize a key for use as a filename
fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // =========================================================================
    // validate_image
    // =========================================================================

    fn make_valid_jpeg() -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        data.extend(vec![0x00; 200]); // pad to pass MIN_IMAGE_SIZE
        data
    }

    fn make_valid_png() -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend(vec![0x00; 200]);
        data
    }

    fn make_valid_gif() -> Vec<u8> {
        let mut data = b"GIF89a".to_vec();
        data.extend(vec![0x00; 200]);
        data
    }

    fn make_valid_webp() -> Vec<u8> {
        let mut data = b"RIFF".to_vec();
        data.extend(vec![0x00; 4]); // size placeholder
        data.extend(b"WEBP");
        data.extend(vec![0x00; 200]);
        data
    }

    fn make_valid_bmp() -> Vec<u8> {
        let mut data = b"BM".to_vec();
        data.extend(vec![0x00; 200]);
        data
    }

    #[test]
    fn validate_image_jpeg() {
        assert!(matches!(
            validate_image(&make_valid_jpeg()),
            ImageValidation::Valid
        ));
    }

    #[test]
    fn validate_image_png() {
        assert!(matches!(
            validate_image(&make_valid_png()),
            ImageValidation::Valid
        ));
    }

    #[test]
    fn validate_image_gif() {
        assert!(matches!(
            validate_image(&make_valid_gif()),
            ImageValidation::Valid
        ));
    }

    #[test]
    fn validate_image_webp() {
        assert!(matches!(
            validate_image(&make_valid_webp()),
            ImageValidation::Valid
        ));
    }

    #[test]
    fn validate_image_bmp() {
        assert!(matches!(
            validate_image(&make_valid_bmp()),
            ImageValidation::Valid
        ));
    }

    #[test]
    fn validate_image_too_small() {
        assert!(matches!(
            validate_image(&[0xFF, 0xD8, 0xFF]),
            ImageValidation::TooSmall(3)
        ));
    }

    #[test]
    fn validate_image_empty() {
        assert!(matches!(validate_image(&[]), ImageValidation::TooSmall(0)));
    }

    #[test]
    fn validate_image_html_response() {
        let mut data = b"<!DOCTYPE html><html>error</html>".to_vec();
        data.extend(vec![0x00; 200]);
        assert!(matches!(validate_image(&data), ImageValidation::NotAnImage));
    }

    #[test]
    fn validate_image_unknown_format() {
        let data = vec![0x00; 200];
        assert!(matches!(
            validate_image(&data),
            ImageValidation::InvalidFormat
        ));
    }

    // =========================================================================
    // sanitize_key
    // =========================================================================

    #[test]
    fn sanitize_key_normal() {
        assert_eq!(sanitize_key("my_image_key"), "my_image_key");
    }

    #[test]
    fn sanitize_key_slashes() {
        assert_eq!(sanitize_key("path/to\\file"), "path_to_file");
    }

    #[test]
    fn sanitize_key_special_chars() {
        assert_eq!(sanitize_key("file:name*?.txt"), "file_name__.txt");
    }

    #[test]
    fn sanitize_key_empty() {
        assert_eq!(sanitize_key(""), "");
    }

    #[test]
    fn fetch_sync_reads_callback_body_through_the_configured_limit() {
        const LIMIT: usize = 8;
        let manager = ResourceManager::without_cache(ResourceConfig {
            default_strategy: StorageStrategy::NoStore,
            max_resource_size: Some(LIMIT),
            ..ResourceConfig::default()
        });
        let request = ResourceRequest::from_url("bounded", "https://example.invalid/body");

        let error = manager
            .fetch_sync(&request, |_| Ok(Cursor::new(vec![0_u8; LIMIT + 1])))
            .expect_err("oversized callback reader must be rejected while reading");

        assert!(error.contains("8-byte materialized read limit"));
    }
}
