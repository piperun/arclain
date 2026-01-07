use anyhow::Result;
use arclain_core::CacheService;
use arclain_db::cache::cache_index::CacheType;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::debug;

#[derive(Clone)]
pub struct ContentCache {
    base_dir: PathBuf,
    service: Arc<CacheService>,
}

impl ContentCache {
    pub fn new(base_dir: PathBuf, service: Arc<CacheService>) -> Result<Self> {
        Ok(Self { base_dir, service })
    }

    pub fn put(
        &self,
        key: &str,
        data: &[u8],
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<String> {
        let sri = cacache::write_hash_sync(&self.base_dir, data)?;

        self.service.upsert(
            key,
            product_id,
            &sri.to_string(),
            source_url,
            cache_type,
            Some(data.len() as i64),
        )?;

        debug!("Cached {} bytes for key {}", data.len(), key);
        Ok(sri.to_string())
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if let Some(entry) = self.service.get(key)? {
            // Guard against empty/invalid hash which causes ssri to panic
            if entry.content_hash.is_empty() {
                tracing::warn!("Found empty content_hash for key: {}", key);
                return Ok(None);
            }

            // Parse the SRI hash
            let sri: ssri::Integrity = match entry.content_hash.parse() {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to parse SRI hash for key {}: {}", key, e);
                    return Ok(None);
                }
            };

            // Read content
            match cacache::read_hash_sync(&self.base_dir, &sri) {
                Ok(data) => {
                    // Update access time
                    if let Err(e) = self.service.update_last_accessed(key) {
                        debug!("Failed to update access time for {}: {}", key, e);
                    }
                    Ok(Some(data))
                }
                Err(cacache::Error::EntryNotFound(_, _)) => {
                    // Entry in DB but not on disk (inconsistent state)
                    Ok(None)
                }
                Err(e) => Err(e.into()),
            }
        } else {
            Ok(None)
        }
    }

    pub fn has(&self, key: &str) -> Result<bool> {
        self.service.has(key)
    }

    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    pub fn remove(&self, key: &str) -> Result<bool> {
        // Remove from DB first to invalidate
        let removed = self.service.delete(key)?;
        // We leave the content in cacache (garbage collection can handle orphaned blobs later)
        Ok(removed)
    }
}
