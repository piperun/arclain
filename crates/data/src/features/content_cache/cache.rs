use crate::traits::CacheIndex;
use anyhow::Result;
use arclain_db::CacheType;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use tracing::{debug, warn};

/// Request to write to cache (sent via channel for serialization)
struct CacheWriteRequest {
    key: String,
    data: Vec<u8>,
    cache_type: CacheType,
    product_id: Option<String>,
    source_url: Option<String>,
}

#[derive(Clone)]
pub struct ContentCache {
    base_dir: PathBuf,
    service: Arc<dyn CacheIndex>,
    write_sender: Sender<CacheWriteRequest>,
}

impl ContentCache {
    pub fn new(base_dir: PathBuf, service: Arc<dyn CacheIndex>) -> Result<Self> {
        // Create channel for write queue
        let (tx, rx) = mpsc::channel::<CacheWriteRequest>();

        // Clone for background thread
        let bg_base_dir = base_dir.clone();
        let bg_service = service.clone();

        // Spawn background writer thread
        thread::Builder::new()
            .name("cache-writer".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    // Write to cacache first (content-addressed, so safe)
                    match cacache::write_hash_sync(&bg_base_dir, &req.data) {
                        Ok(sri) => {
                            // Retry upsert up to 3 times on database lock
                            let mut attempts = 0;
                            loop {
                                attempts += 1;
                                match bg_service.upsert(
                                    &req.key,
                                    req.product_id.as_deref(),
                                    &sri.to_string(),
                                    req.source_url.as_deref(),
                                    req.cache_type.clone(),
                                    Some(req.data.len() as i64),
                                ) {
                                    Ok(_) => {
                                        debug!(
                                            "Cached {} bytes for key {}",
                                            req.data.len(),
                                            req.key
                                        );
                                        break;
                                    }
                                    Err(e) => {
                                        if attempts < 3 {
                                            // Brief pause before retry
                                            thread::sleep(std::time::Duration::from_millis(
                                                50 * attempts as u64,
                                            ));
                                        } else {
                                            warn!(
                                                "Failed to cache {} after {} attempts: {}",
                                                req.key, attempts, e
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to write content to cacache for {}: {}", req.key, e);
                        }
                    }
                }
                debug!("Cache writer thread exiting");
            })?;

        Ok(Self {
            base_dir,
            service,
            write_sender: tx,
        })
    }

    /// Queue a write operation (non-blocking, for async contexts)
    /// The write happens in a background thread to avoid SQLite lock contention
    pub fn queue_put(
        &self,
        key: &str,
        data: Vec<u8>,
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) {
        let req = CacheWriteRequest {
            key: key.to_string(),
            data,
            cache_type,
            product_id: product_id.map(|s| s.to_string()),
            source_url: source_url.map(|s| s.to_string()),
        };

        if let Err(e) = self.write_sender.send(req) {
            warn!("Failed to queue cache write for {}: {}", key, e);
        }
    }

    /// Synchronous put with retry logic (for cases where blocking is acceptable)
    pub fn put(
        &self,
        key: &str,
        data: &[u8],
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<String> {
        let sri = cacache::write_hash_sync(&self.base_dir, data)?;

        // Retry upsert up to 3 times on database lock
        // Audit finding H7: the original code ended with
        // `Err(last_error.unwrap())`. Today every loop iteration that
        // doesn't return Ok sets `last_error`, so the unwrap is safe —
        // but it's a brittle invariant. A future refactor that adds an
        // Ok-without-return path would silently start panicking. Use
        // unwrap_or_else with an explicit fallback message instead.
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3 {
            match self.service.upsert(
                key,
                product_id,
                &sri.to_string(),
                source_url,
                cache_type.clone(),
                Some(data.len() as i64),
            ) {
                Ok(_) => {
                    debug!("Cached {} bytes for key {}", data.len(), key);
                    return Ok(sri.to_string());
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < 3 {
                        thread::sleep(std::time::Duration::from_millis(50 * attempt as u64));
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "content cache upsert failed for key {} after 3 attempts \
                 (with no recorded error — likely a control-flow bug)",
                key
            )
        }))
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if let Some(entry) = self.service.get(key)? {
            // Guard against empty/invalid hash which causes ssri to panic
            if entry.content_hash.is_empty() {
                warn!("Found empty content_hash for key: {}", key);
                return Ok(None);
            }

            // Parse the SRI hash
            let sri: ssri::Integrity = match entry.content_hash.parse() {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to parse SRI hash for key {}: {}", key, e);
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

    pub fn remove_by_pattern(&self, pattern: &str) -> Result<usize> {
        // Remove from DB first to invalidate matching keys
        let removed_count = self.service.delete_by_pattern(pattern)?;
        // We leave the content in cacache
        Ok(removed_count)
    }
}
