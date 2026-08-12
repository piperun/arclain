use super::key_lock::{cache_key_lock, cache_root_lock, CacheKeyLock, CacheRootLock};
use super::{CacheLimits, CacheOwner, CacheQuota};
use crate::traits::CacheIndex;
use crate::{
    shared::{read_to_end_with_limit, safe_log_fingerprint},
    ResourceConfig, DEFAULT_MAX_RESOURCE_SIZE_BYTES,
};
use anyhow::{bail, Context, Result};
use arclain_db::{CacheEntry, CacheType};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;
use tracing::{debug, warn};

/// Crate-private streaming write handle into `ContentCache`. Bytes flow
/// through `Write`; only `ContentCache` may finalize the blob so the physical
/// commit and index upsert stay inside one cache-root critical section.
///
/// Dropping without committing leaves the cacache temp file orphaned;
/// cacache's `NamedTempFile` cleans that up on drop, so no manual
/// cleanup is required on the abort path.
pub(crate) struct StreamingWriter {
    inner: cacache::SyncWriter,
    bytes_written: u64,
}

impl Write for StreamingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl StreamingWriter {
    fn commit_raw(self) -> Result<(String, u64)> {
        let bytes = self.bytes_written;
        let sri = self.inner.commit()?;
        Ok((sri.to_string(), bytes))
    }
}

/// Request to write to cache (sent via channel for serialization)
struct CacheWriteRequest {
    owner: CacheOwner,
    key: String,
    data: Vec<u8>,
    cache_type: CacheType,
    product_id: Option<String>,
    source_url: Option<String>,
    reservation: ReservationGuard,
    _queue_permit: QueuePermit,
}

struct ReservationGuard {
    quota: Arc<CacheQuota>,
    path: PathBuf,
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        let _ = self.quota.release(&self.path);
    }
}

#[derive(Default)]
struct QueueBudgetState {
    items: usize,
    bytes: u64,
}

struct QueueBudget {
    max_items: usize,
    max_bytes: u64,
    state: Mutex<QueueBudgetState>,
}

impl QueueBudget {
    fn acquire(self: &Arc<Self>, bytes: u64) -> Result<QueuePermit> {
        let mut state = self.state.lock();
        if state.items >= self.max_items {
            bail!("cache queue item quota exceeded");
        }
        if state.bytes.saturating_add(bytes) > self.max_bytes {
            bail!("cache queue byte quota exceeded");
        }
        state.items += 1;
        state.bytes = state.bytes.saturating_add(bytes);
        Ok(QueuePermit {
            budget: self.clone(),
            bytes,
        })
    }
}

struct QueuePermit {
    budget: Arc<QueueBudget>,
    bytes: u64,
}

impl Drop for QueuePermit {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock();
        state.items = state.items.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
    }
}

fn buffered_reservation_path(base_dir: &Path) -> PathBuf {
    static BUFFERED_RESERVATION_COUNTER: AtomicUsize = AtomicUsize::new(0);
    base_dir.join(".partial").join(format!(
        ".buffered-{}-{}.reservation",
        std::process::id(),
        BUFFERED_RESERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Fetch the index row a write is about to replace so its blob can be
/// discarded once unreferenced. Gated on an EXISTS probe: the common
/// fresh-write path costs no index row fetch, keeping cache writes at
/// zero `CacheIndex::get` calls. The caller holds the scoped key lock,
/// so the has → get pair cannot race another writer of the same key; a
/// concurrent pattern-delete or quota eviction that removes the row in
/// between performs its own blob discard, so no orphan is leaked.
fn probe_replaced_entry(service: &dyn CacheIndex, scoped_key: &str) -> Result<Option<CacheEntry>> {
    if service.has(scoped_key)? {
        service.get(scoped_key)
    } else {
        Ok(None)
    }
}

fn put_for_owner_with(
    base_dir: &PathBuf,
    service: &dyn CacheIndex,
    quota: &CacheQuota,
    owner: &CacheOwner,
    key: &str,
    data: &[u8],
    cache_type: CacheType,
    product_id: Option<&str>,
    source_url: Option<&str>,
    pre_reserved_path: Option<&Path>,
) -> Result<String> {
    let scoped_key = owner.scoped_key(key);
    let key_lock = cache_key_lock(base_dir, &scoped_key)?;
    let _key_guard = key_lock.lock();
    let owned_reservation_path;
    let reservation_path = if let Some(path) = pre_reserved_path {
        path
    } else {
        owned_reservation_path = buffered_reservation_path(base_dir);
        &owned_reservation_path
    };
    let bytes = u64::try_from(data.len()).context("buffered cache object is too large")?;
    let size_bytes = i64::try_from(bytes).context("cache byte count does not fit SQLite")?;
    if pre_reserved_path.is_none() {
        quota.reserve(
            base_dir,
            service,
            owner,
            &scoped_key,
            reservation_path,
            bytes,
        )?;
    }
    let commit_admission = match quota.prepare_commit(base_dir, service, owner, &scoped_key, bytes)
    {
        Ok(admission) => admission,
        Err(error) => {
            let _ = quota.release(reservation_path);
            return Err(error);
        }
    };

    let previous_entry = match probe_replaced_entry(service, &scoped_key) {
        Ok(entry) => entry,
        Err(error) => {
            let _ = quota.release(reservation_path);
            return Err(error);
        }
    };
    let root_lock = cache_root_lock(base_dir)?;
    let root_guard = root_lock.lock();
    let sri = match cacache::write_hash_sync(base_dir, data) {
        Ok(sri) => sri,
        Err(error) => {
            let _ = quota.release(reservation_path);
            return Err(error.into());
        }
    };
    let sri_string = sri.to_string();

    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=3 {
        match service.upsert(
            &scoped_key,
            product_id,
            &sri_string,
            source_url,
            cache_type,
            Some(size_bytes),
        ) {
            Ok(_) => {
                drop(commit_admission);
                drop(root_guard);
                debug!(
                    "Cached {} bytes for key {}",
                    data.len(),
                    safe_log_fingerprint(&scoped_key)
                );
                if let Some(previous) = previous_entry
                    .as_ref()
                    .filter(|entry| entry.content_hash != sri_string)
                {
                    let _ = quota.discard_blob_if_unreferenced(
                        base_dir,
                        service,
                        &previous.content_hash,
                    );
                }
                quota.release(reservation_path)?;
                return Ok(sri_string);
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < 3 {
                    thread::sleep(std::time::Duration::from_millis(50 * attempt as u64));
                }
            }
        }
    }

    drop(root_guard);
    let error = last_error.unwrap_or_else(|| {
        anyhow::anyhow!(
            "content cache upsert failed for key {} after 3 attempts \
             (with no recorded error — likely a control-flow bug)",
            scoped_key
        )
    });
    let _ = quota.discard_blob_if_unreferenced(base_dir, service, &sri_string);
    let _ = quota.release(reservation_path);
    Err(error)
}

#[derive(Clone)]
pub struct ContentCache {
    base_dir: PathBuf,
    service: Arc<dyn CacheIndex>,
    write_sender: SyncSender<CacheWriteRequest>,
    quota: Arc<CacheQuota>,
    queue_budget: Arc<QueueBudget>,
}

impl ContentCache {
    pub fn new(base_dir: PathBuf, service: Arc<dyn CacheIndex>) -> Result<Self> {
        Self::new_with_limits(base_dir, service, CacheLimits::default())
    }

    pub fn new_with_config(
        base_dir: PathBuf,
        service: Arc<dyn CacheIndex>,
        config: &ResourceConfig,
    ) -> Result<Self> {
        Self::new_with_limits(base_dir, service, config.cache_limits.clone())
    }

    pub fn new_with_limits(
        base_dir: PathBuf,
        service: Arc<dyn CacheIndex>,
        limits: CacheLimits,
    ) -> Result<Self> {
        std::fs::create_dir_all(&base_dir).context("creating content cache directory")?;
        // Create channel for write queue
        let (tx, rx) = mpsc::sync_channel::<CacheWriteRequest>(limits.max_queued_writes.max(1));

        // Clone for background thread
        let bg_base_dir = base_dir.clone();
        let bg_service = service.clone();
        let quota = Arc::new(CacheQuota::new(limits));
        quota.maintain(&base_dir, service.as_ref())?;
        let bg_quota = quota.clone();
        let queue_budget = Arc::new(QueueBudget {
            max_items: quota.limits().max_queued_writes,
            max_bytes: quota.limits().max_queued_bytes,
            state: Mutex::new(QueueBudgetState::default()),
        });

        // Spawn background writer thread
        thread::Builder::new()
            .name("cache-writer".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    if let Err(error) = put_for_owner_with(
                        &bg_base_dir,
                        bg_service.as_ref(),
                        bg_quota.as_ref(),
                        &req.owner,
                        &req.key,
                        &req.data,
                        req.cache_type,
                        req.product_id.as_deref(),
                        req.source_url.as_deref(),
                        Some(&req.reservation.path),
                    ) {
                        warn!(
                            "Failed to cache queued key {}: {}",
                            safe_log_fingerprint(&req.key),
                            safe_log_fingerprint(format!("{error:#}"))
                        );
                    }
                }
                debug!("Cache writer thread exiting");
            })?;

        Ok(Self {
            base_dir,
            service,
            write_sender: tx,
            quota,
            queue_budget,
        })
    }

    pub fn limits(&self) -> &CacheLimits {
        self.quota.limits()
    }

    pub(crate) fn cache_index(&self) -> &dyn CacheIndex {
        self.service.as_ref()
    }

    pub(crate) fn quota(&self) -> &CacheQuota {
        self.quota.as_ref()
    }

    pub(crate) fn key_lock(&self, scoped_key: &str) -> Result<CacheKeyLock> {
        cache_key_lock(&self.base_dir, scoped_key)
    }

    pub(crate) fn root_lock(&self) -> Result<CacheRootLock> {
        cache_root_lock(&self.base_dir)
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
    ) -> Result<()> {
        self.queue_put_for_owner(
            &CacheOwner::host(),
            key,
            data,
            cache_type,
            product_id,
            source_url,
        )
    }

    pub fn queue_put_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        data: Vec<u8>,
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<()> {
        let bytes = u64::try_from(data.len()).context("queued cache object is too large")?;
        let queue_permit = self.queue_budget.acquire(bytes)?;
        let scoped_key = owner.scoped_key(key);
        let reservation_path = buffered_reservation_path(&self.base_dir);
        self.quota.reserve(
            &self.base_dir,
            self.service.as_ref(),
            owner,
            &scoped_key,
            &reservation_path,
            bytes,
        )?;
        let req = CacheWriteRequest {
            owner: owner.clone(),
            key: key.to_string(),
            data,
            cache_type,
            product_id: product_id.map(|s| s.to_string()),
            source_url: source_url.map(|s| s.to_string()),
            reservation: ReservationGuard {
                quota: self.quota.clone(),
                path: reservation_path,
            },
            _queue_permit: queue_permit,
        };

        match self.write_sender.try_send(req) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_request)) => bail!("cache queue channel is full"),
            Err(TrySendError::Disconnected(_request)) => {
                bail!("cache queue worker is unavailable")
            }
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
        self.put_for_owner(
            &CacheOwner::host(),
            key,
            data,
            cache_type,
            product_id,
            source_url,
        )
    }

    pub fn put_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        data: &[u8],
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<String> {
        put_for_owner_with(
            &self.base_dir,
            self.service.as_ref(),
            self.quota.as_ref(),
            owner,
            key,
            data,
            cache_type,
            product_id,
            source_url,
            None,
        )
    }

    /// Open a crate-private streaming write into the cache. The returned
    /// writer hashes bytes as they flow through without materializing the
    /// whole object in RAM. It must be finalized through
    /// [`Self::commit_streaming_for_owner_locked`].
    ///
    /// Drop without `commit` cleans up the temp file via cacache's
    /// `NamedTempFile`; no index entry is written.
    pub(crate) fn open_streaming_writer(&self) -> Result<StreamingWriter> {
        // open_hash_sync(cache) keeps `key: None` so commit() returns
        // the SRI without touching cacache's native key index — we
        // manage the key → SRI mapping in our own SQLite index.
        let inner = cacache::WriteOpts::new()
            .algorithm(cacache::Algorithm::Sha256)
            .open_hash_sync(&self.base_dir)?;
        Ok(StreamingWriter {
            inner,
            bytes_written: 0,
        })
    }

    /// Finalize a streaming blob and publish its index row atomically with
    /// respect to cache reconciliation. The caller must already hold the
    /// scoped key lock; the streaming downloader does so across its partial
    /// resume lifecycle.
    pub(crate) fn commit_streaming_for_owner_locked(
        &self,
        owner: &CacheOwner,
        key: &str,
        writer: StreamingWriter,
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<(String, u64)> {
        let scoped_key = owner.scoped_key(key);
        let root_lock = self.root_lock()?;
        let root_guard = root_lock.lock();
        let previous_entry = probe_replaced_entry(self.service.as_ref(), &scoped_key)?;
        let (sri, bytes) = writer.commit_raw()?;
        if let Err(error) = self.upsert_sri_for_owner_unlocked(
            owner, key, &sri, bytes, cache_type, product_id, source_url,
        ) {
            let _ = CacheQuota::discard_blob_if_unreferenced_under_root(
                &self.base_dir,
                self.service.as_ref(),
                &sri,
            );
            return Err(error);
        }
        drop(root_guard);

        if let Some(previous) = previous_entry.filter(|entry| entry.content_hash != sri) {
            let _ = self.quota.discard_blob_if_unreferenced(
                &self.base_dir,
                self.service.as_ref(),
                &previous.content_hash,
            );
        }
        Ok((sri, bytes))
    }

    pub(crate) fn upsert_sri_for_owner_unlocked(
        &self,
        owner: &CacheOwner,
        key: &str,
        sri: &str,
        bytes: u64,
        cache_type: CacheType,
        product_id: Option<&str>,
        source_url: Option<&str>,
    ) -> Result<()> {
        let scoped_key = owner.scoped_key(key);
        let size_bytes = i64::try_from(bytes).context("cache byte count does not fit SQLite")?;
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 1..=3 {
            match self.service.upsert(
                &scoped_key,
                product_id,
                sri,
                source_url,
                cache_type,
                Some(size_bytes),
            ) {
                Ok(_) => {
                    debug!(
                        "Streamed {} bytes for key {} → {}",
                        bytes,
                        safe_log_fingerprint(&scoped_key),
                        safe_log_fingerprint(sri)
                    );
                    return Ok(());
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
                 (streaming path)",
                scoped_key
            )
        }))
    }

    /// Read a cache entry into memory using the canonical materialized-resource
    /// ceiling. Large cache entries remain available to streaming consumers.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.get_for_owner(&CacheOwner::host(), key)
    }

    pub fn get_for_owner(&self, owner: &CacheOwner, key: &str) -> Result<Option<Vec<u8>>> {
        self.get_with_limit_for_owner(owner, key, DEFAULT_MAX_RESOURCE_SIZE_BYTES)
    }

    /// Read a cache entry into memory without allowing the returned allocation
    /// to grow beyond `limit` bytes.
    ///
    /// Indexed size metadata provides an early rejection, but the content is
    /// still read through a bounded stream because legacy or stale rows may
    /// have no trustworthy size.
    pub fn get_with_limit(&self, key: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        self.get_with_limit_for_owner(&CacheOwner::host(), key, limit)
    }

    pub fn get_with_limit_for_owner(
        &self,
        owner: &CacheOwner,
        key: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>> {
        let scoped_key = owner.scoped_key(key);
        if let Some(data) = self.get_indexed_with_limit(&scoped_key, limit)? {
            return Ok(Some(data));
        }
        // Rows written before owner scoping live under the raw key. Nothing
        // writes raw host rows anymore, so gate the fallback on an EXISTS
        // probe: a plain miss stays a single `CacheIndex::get`, which the
        // image pipeline relies on to prove it never polls the index.
        if matches!(owner, CacheOwner::Host) && self.service.has(key)? {
            return self.get_indexed_with_limit(key, limit);
        }
        Ok(None)
    }

    fn get_indexed_with_limit(&self, index_key: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        if let Some(entry) = self.service.get(index_key)? {
            if entry.size_bytes.is_some_and(|size| {
                size >= 0 && usize::try_from(size).map_or(true, |size| size > limit)
            }) {
                bail!("cached content exceeds the {limit}-byte materialized read limit");
            }

            // Guard against empty/invalid hash which causes ssri to panic
            if entry.content_hash.is_empty() {
                warn!(
                    "Found empty content_hash for key: {}",
                    safe_log_fingerprint(index_key)
                );
                return Ok(None);
            }

            // Parse the SRI hash
            let sri: ssri::Integrity = match entry.content_hash.parse() {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        "Failed to parse SRI hash for key {}: {}",
                        safe_log_fingerprint(index_key),
                        safe_log_fingerprint(e.to_string())
                    );
                    return Ok(None);
                }
            };

            // Stream content through the hard ceiling even when indexed size
            // metadata is absent or stale.
            match cacache::SyncReader::open_hash(&self.base_dir, sri) {
                Ok(mut reader) => {
                    let data = read_to_end_with_limit(&mut reader, limit, "cached content")?;
                    reader.check()?;
                    // Update access time
                    if let Err(e) = self.service.update_last_accessed(index_key) {
                        debug!(
                            "Failed to update access time for {}: {}",
                            safe_log_fingerprint(index_key),
                            safe_log_fingerprint(e.to_string())
                        );
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
        self.has_for_owner(&CacheOwner::host(), key)
    }

    pub fn has_for_owner(&self, owner: &CacheOwner, key: &str) -> Result<bool> {
        if self.service.has(&owner.scoped_key(key))? {
            return Ok(true);
        }
        if matches!(owner, CacheOwner::Host) {
            return self.service.has(key);
        }
        Ok(false)
    }

    /// Count entries under one raw-key prefix inside exactly one owner.
    pub fn count_keys_with_prefix_for_owner(
        &self,
        owner: &CacheOwner,
        prefix: &str,
        cache_type: CacheType,
    ) -> Result<u64> {
        self.service
            .count_keys_with_prefix(&owner.scoped_key(prefix), cache_type)
    }

    /// Return one deterministic raw-key page from exactly one owner.
    pub fn list_keys_with_prefix_page_for_owner(
        &self,
        owner: &CacheOwner,
        prefix: &str,
        cache_type: CacheType,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<String>> {
        let scoped_prefix = owner.scoped_key(prefix);
        self.service
            .list_keys_with_prefix_page(&scoped_prefix, cache_type, offset, limit)?
            .into_iter()
            .map(|scoped_key| {
                let (actual_owner, raw_key) = super::quota::parse_scoped_key(&scoped_key)
                    .context("cache index returned an invalid scoped key")?;
                if actual_owner != *owner || !raw_key.starts_with(prefix) {
                    bail!("cache index returned a key outside the requested owner prefix");
                }
                Ok(raw_key.to_string())
            })
            .collect()
    }

    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    /// Removes the physical content-addressed blobs and unfinished
    /// streaming writes owned by this cache, plus their index rows,
    /// without deleting sibling application directories under the same
    /// root (resources, materializations, and so on).
    ///
    /// The index is cleared first, under the same root lock. If physical
    /// deletion then fails, unreferenced blobs may leak but no live row can
    /// point at a missing blob and turn an ordinary cache miss into I/O
    /// failure.
    pub fn clear_content(&self) -> Result<()> {
        let root_lock = self.root_lock()?;
        let _root_guard = root_lock.lock();
        self.service
            .clear_all()
            .context("clearing content cache index")?;
        for directory in ["content-v2", ".partial"] {
            let path = self.base_dir.join(directory);
            if path.exists() {
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("clearing content cache directory {path:?}"))?;
            }
        }
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<bool> {
        self.remove_for_owner(&CacheOwner::host(), key)
    }

    pub fn remove_for_owner(&self, owner: &CacheOwner, key: &str) -> Result<bool> {
        let scoped_key = owner.scoped_key(key);
        let key_lock = self.key_lock(&scoped_key)?;
        let _key_guard = key_lock.lock();
        let root_lock = self.root_lock()?;
        let _root_guard = root_lock.lock();
        // Rows written before owner scoping live under the raw key; nothing
        // writes raw host rows anymore, so one EXISTS probe decides whether
        // the legacy keyspace needs touching at all.
        let legacy_row_present = matches!(owner, CacheOwner::Host) && self.service.has(key)?;
        let mut removed_hashes = HashSet::new();
        // `discard_blob_if_unreferenced_under_root` fails closed (treats
        // every blob as referenced) when the index cannot enumerate itself,
        // so for incomplete-view indexes the hash probes below could never
        // lead to a physical discard. Skip the dead row fetches; a removal
        // then costs exactly one index delete.
        if self.service.has_complete_lru_view() {
            if let Some(entry) = self.service.get(&scoped_key)? {
                removed_hashes.insert(entry.content_hash);
            }
            if legacy_row_present {
                if let Some(entry) = self.service.get(key)? {
                    removed_hashes.insert(entry.content_hash);
                }
            }
        }

        let mut removed = self.service.delete(&scoped_key)?;
        if legacy_row_present {
            removed |= self.service.delete(key)?;
        }
        for content_hash in removed_hashes {
            CacheQuota::discard_blob_if_unreferenced_under_root(
                &self.base_dir,
                self.service.as_ref(),
                &content_hash,
            )?;
        }
        Ok(removed)
    }

    pub fn remove_by_pattern(&self, pattern: &str) -> Result<usize> {
        self.remove_by_pattern_for_owner(&CacheOwner::host(), pattern)
    }

    pub fn remove_by_pattern_for_owner(&self, owner: &CacheOwner, pattern: &str) -> Result<usize> {
        let root_lock = self.root_lock()?;
        let _root_guard = root_lock.lock();
        let candidate_hashes: HashSet<_> = self.service.content_hashes()?.into_iter().collect();
        let mut removed_count = self.service.delete_by_pattern(&owner.scoped_key(pattern))?;
        if matches!(owner, CacheOwner::Host) {
            removed_count = removed_count.saturating_add(self.service.delete_by_pattern(pattern)?);
        }
        for content_hash in candidate_hashes {
            CacheQuota::discard_blob_if_unreferenced_under_root(
                &self.base_dir,
                self.service.as_ref(),
                &content_hash,
            )?;
        }
        Ok(removed_count)
    }
}

#[cfg(test)]
mod quota_api_tests {
    use super::*;
    use crate::features::content_cache::{CacheLimits, CacheOwner};
    use arclain_db::CacheEntry;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Barrier};

    #[derive(Default)]
    struct MapIndex {
        entries: Mutex<HashMap<String, CacheEntry>>,
    }

    struct RacingIndex {
        entry: Mutex<Option<CacheEntry>>,
        upsert_calls: AtomicUsize,
        second_upsert_entered: mpsc::Sender<()>,
        wait_for_second_upsert: Mutex<Option<mpsc::Receiver<()>>>,
    }

    struct BlockingQueueIndex {
        inner: MapIndex,
        first_upsert_started: Mutex<Option<mpsc::Sender<()>>>,
        release_first_upsert: Mutex<Option<mpsc::Receiver<()>>>,
    }

    struct AdmissionWindowIndex {
        inner: MapIndex,
        first_probe_started: Mutex<Option<mpsc::Sender<()>>>,
        release_first_probe: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl CacheIndex for AdmissionWindowIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            self.inner.upsert(
                key,
                product_id,
                content_hash,
                source_url,
                cache_type,
                size_bytes,
            )
        }

        fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
            self.inner.get(key)
        }

        // A fresh-key put's first index probe after `prepare_commit` is the
        // replaced-entry EXISTS check, so holding it here keeps the first put
        // inside its admission window while the second put races admission.
        fn has(&self, key: &str) -> Result<bool> {
            let started = self.first_probe_started.lock().take();
            if let Some(started) = started {
                started.send(()).unwrap();
                let release = self
                    .release_first_probe
                    .lock()
                    .take()
                    .expect("admission-window release receiver");
                release.recv().unwrap();
            }
            self.inner.has(key)
        }
        fn delete(&self, key: &str) -> Result<bool> {
            self.inner.delete(key)
        }
        fn delete_by_pattern(&self, pattern: &str) -> Result<usize> {
            self.inner.delete_by_pattern(pattern)
        }
        fn update_last_accessed(&self, key: &str) -> Result<()> {
            self.inner.update_last_accessed(key)
        }
        fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
            self.inner.entries_lru()
        }
        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    impl CacheIndex for BlockingQueueIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            let first_upsert_started = self.first_upsert_started.lock().take();
            if let Some(started) = first_upsert_started {
                started.send(()).unwrap();
                let release_first_upsert = self.release_first_upsert.lock().take().unwrap();
                release_first_upsert.recv().unwrap();
            }
            self.inner.upsert(
                key,
                product_id,
                content_hash,
                source_url,
                cache_type,
                size_bytes,
            )
        }

        fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
            self.inner.get(key)
        }
        fn has(&self, key: &str) -> Result<bool> {
            self.inner.has(key)
        }
        fn delete(&self, key: &str) -> Result<bool> {
            self.inner.delete(key)
        }
        fn delete_by_pattern(&self, pattern: &str) -> Result<usize> {
            self.inner.delete_by_pattern(pattern)
        }
        fn update_last_accessed(&self, key: &str) -> Result<()> {
            self.inner.update_last_accessed(key)
        }
        fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
            self.inner.entries_lru()
        }
        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    impl CacheIndex for RacingIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            let call = self.upsert_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                let _ = self
                    .wait_for_second_upsert
                    .lock()
                    .take()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_millis(250));
            } else {
                let _ = self.second_upsert_entered.send(());
            }
            *self.entry.lock() = Some(CacheEntry {
                id: call as i64 + 1,
                key: key.to_string(),
                product_id: product_id.map(str::to_string),
                content_hash: content_hash.to_string(),
                source_url: source_url.map(str::to_string),
                cache_type,
                created_at: format!("{call:020}"),
                last_accessed: None,
                size_bytes,
            });
            Ok(call as i64 + 1)
        }

        fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
            Ok(self
                .entry
                .lock()
                .as_ref()
                .filter(|entry| entry.key == key)
                .cloned())
        }

        fn has(&self, key: &str) -> Result<bool> {
            Ok(self
                .entry
                .lock()
                .as_ref()
                .is_some_and(|entry| entry.key == key))
        }

        fn delete(&self, key: &str) -> Result<bool> {
            let mut entry = self.entry.lock();
            if entry.as_ref().is_some_and(|entry| entry.key == key) {
                entry.take();
                return Ok(true);
            }
            Ok(false)
        }

        fn delete_by_pattern(&self, _pattern: &str) -> Result<usize> {
            Ok(0)
        }

        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }

        fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
            Ok(self.entry.lock().iter().cloned().collect())
        }

        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    fn count_content_files(path: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .map(|path| {
                if path.is_dir() {
                    count_content_files(&path)
                } else {
                    1
                }
            })
            .sum()
    }

    impl CacheIndex for MapIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            let mut entries = self.entries.lock();
            let id = entries.len() as i64 + 1;
            entries.insert(
                key.to_string(),
                CacheEntry {
                    id,
                    key: key.to_string(),
                    product_id: product_id.map(str::to_string),
                    content_hash: content_hash.to_string(),
                    source_url: source_url.map(str::to_string),
                    cache_type,
                    created_at: format!("{id:020}"),
                    last_accessed: None,
                    size_bytes,
                },
            );
            Ok(id)
        }

        fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
            Ok(self.entries.lock().get(key).cloned())
        }

        fn has(&self, key: &str) -> Result<bool> {
            Ok(self.entries.lock().contains_key(key))
        }

        fn delete(&self, key: &str) -> Result<bool> {
            Ok(self.entries.lock().remove(key).is_some())
        }

        fn delete_by_pattern(&self, pattern: &str) -> Result<usize> {
            let prefix = pattern.strip_suffix('*').unwrap_or(pattern);
            let mut entries = self.entries.lock();
            let before = entries.len();
            entries.retain(|key, _| !key.starts_with(prefix));
            Ok(before - entries.len())
        }

        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }

        fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
            let mut entries: Vec<_> = self.entries.lock().values().cloned().collect();
            entries.sort_by_key(|entry| entry.id);
            Ok(entries)
        }

        fn count_keys_with_prefix(
            &self,
            scoped_prefix: &str,
            cache_type: CacheType,
        ) -> Result<u64> {
            Ok(self
                .entries
                .lock()
                .values()
                .filter(|entry| {
                    entry.key.starts_with(scoped_prefix) && entry.cache_type == cache_type
                })
                .count() as u64)
        }

        fn list_keys_with_prefix_page(
            &self,
            scoped_prefix: &str,
            cache_type: CacheType,
            offset: usize,
            limit: usize,
        ) -> Result<Vec<String>> {
            let mut keys: Vec<_> = self
                .entries
                .lock()
                .values()
                .filter(|entry| {
                    entry.key.starts_with(scoped_prefix) && entry.cache_type == cache_type
                })
                .map(|entry| entry.key.clone())
                .collect();
            keys.sort();
            Ok(keys.into_iter().skip(offset).take(limit).collect())
        }

        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    fn owner_cache() -> (tempfile::TempDir, ContentCache, Arc<MapIndex>) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        std::fs::create_dir_all(&base).unwrap();
        let index = Arc::new(MapIndex::default());
        let cache = ContentCache::new_with_limits(
            base,
            index.clone(),
            CacheLimits {
                min_free_space_bytes: 0,
                ..CacheLimits::default()
            },
        )
        .unwrap();
        (dir, cache, index)
    }

    #[test]
    fn streaming_index_rejects_byte_counts_that_do_not_fit_sqlite() {
        let (_dir, cache, index) = owner_cache();

        let error = cache
            .upsert_sri_for_owner_unlocked(
                &CacheOwner::host(),
                "oversized",
                "sha256-placeholder",
                u64::MAX,
                CacheType::Other,
                None,
                None,
            )
            .unwrap_err();

        assert!(error.to_string().contains("does not fit SQLite"));
        assert!(!index
            .has(&CacheOwner::host().scoped_key("oversized"))
            .unwrap());
    }

    #[test]
    fn owner_scoped_keys_are_unambiguous_and_isolate_plugins_from_host() {
        let host = CacheOwner::host().scoped_key("shared:key");
        let plugin_a = CacheOwner::plugin("a").scoped_key("shared:key");
        let plugin_b = CacheOwner::plugin("b").scoped_key("shared:key");
        let delimiter_attack = CacheOwner::plugin("a:plugin:b").scoped_key("shared:key");

        assert_ne!(host, plugin_a);
        assert_ne!(plugin_a, plugin_b);
        assert_ne!(plugin_b, delimiter_attack);
        assert_eq!(
            CacheOwner::from_scoped_key(&plugin_a),
            Some(CacheOwner::plugin("a"))
        );
        assert_eq!(CacheOwner::from_scoped_key("legacy:key"), None);
    }

    #[test]
    fn default_streaming_object_quota_preserves_large_video_support() {
        assert!(CacheLimits::default().max_object_bytes > 50 * 1024 * 1024);
    }

    #[test]
    fn clear_content_removes_blobs_and_partials_but_preserves_cache_siblings() {
        let (_dir, cache, _index) = owner_cache();
        cache
            .put("fixture", b"cached body", CacheType::Other, None, None)
            .unwrap();
        let partial_dir = cache.base_dir().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        std::fs::write(partial_dir.join("unfinished"), b"partial").unwrap();
        let resources_dir = cache.base_dir().join("resources");
        std::fs::create_dir_all(&resources_dir).unwrap();
        std::fs::write(resources_dir.join("keep"), b"resource").unwrap();

        cache.clear_content().unwrap();

        assert!(cache.get("fixture").unwrap().is_none());
        assert!(!cache.base_dir().join("content-v2").exists());
        assert!(!partial_dir.exists());
        assert!(resources_dir.join("keep").is_file());
    }

    #[test]
    fn plugin_owned_content_is_invisible_to_host_and_other_plugins() {
        let (_dir, cache, _index) = owner_cache();
        let plugin_a = CacheOwner::plugin("plugin-a");
        let plugin_b = CacheOwner::plugin("plugin-b");

        cache
            .put_for_owner(
                &plugin_a,
                "shared",
                b"private",
                CacheType::Other,
                None,
                None,
            )
            .unwrap();

        assert_eq!(
            cache.get_for_owner(&plugin_a, "shared").unwrap().as_deref(),
            Some(b"private".as_slice())
        );
        assert!(cache.get_for_owner(&plugin_b, "shared").unwrap().is_none());
        assert!(cache.get("shared").unwrap().is_none());
    }

    #[test]
    fn owner_prefix_queries_return_only_raw_keys_for_the_exact_owner() {
        let (_dir, cache, _index) = owner_cache();
        let host = CacheOwner::host();
        let plugin_a = CacheOwner::plugin("plugin-a");
        let plugin_b = CacheOwner::plugin("plugin-b");
        for (owner, key) in [
            (&host, "state:host"),
            (&plugin_a, "state:two"),
            (&plugin_a, "state:one"),
            (&plugin_a, "other:key"),
            (&plugin_b, "state:other"),
        ] {
            cache
                .put_for_owner(owner, key, b"x", CacheType::PluginData, None, None)
                .unwrap();
        }
        cache
            .put_for_owner(
                &plugin_a,
                "state:reserved",
                b"x",
                CacheType::Metadata,
                None,
                None,
            )
            .unwrap();

        assert_eq!(
            cache
                .count_keys_with_prefix_for_owner(&plugin_a, "state:", CacheType::PluginData)
                .unwrap(),
            2
        );
        assert_eq!(
            cache
                .list_keys_with_prefix_page_for_owner(
                    &plugin_a,
                    "state:",
                    CacheType::PluginData,
                    0,
                    1,
                )
                .unwrap(),
            vec!["state:one".to_string()]
        );
        assert_eq!(
            cache
                .list_keys_with_prefix_page_for_owner(
                    &plugin_a,
                    "state:",
                    CacheType::PluginData,
                    1,
                    1,
                )
                .unwrap(),
            vec!["state:two".to_string()]
        );
    }

    #[test]
    fn plugin_delete_pattern_is_confined_to_its_owner_namespace() {
        let (_dir, cache, _index) = owner_cache();
        let plugin_a = CacheOwner::plugin("plugin-a");
        let plugin_b = CacheOwner::plugin("plugin-b");
        for owner in [&plugin_a, &plugin_b] {
            cache
                .put_for_owner(owner, "prefix:item", b"x", CacheType::Other, None, None)
                .unwrap();
        }

        assert_eq!(
            cache
                .remove_by_pattern_for_owner(&plugin_a, "prefix:*")
                .unwrap(),
            1
        );
        assert!(!cache.has_for_owner(&plugin_a, "prefix:item").unwrap());
        assert!(cache.has_for_owner(&plugin_b, "prefix:item").unwrap());
        let shared_hash: ssri::Integrity = cache
            .service
            .get(&plugin_b.scoped_key("prefix:item"))
            .unwrap()
            .unwrap()
            .content_hash
            .parse()
            .unwrap();
        assert!(cacache::SyncReader::open_hash(cache.base_dir(), shared_hash.clone()).is_ok());

        assert_eq!(
            cache
                .remove_by_pattern_for_owner(&plugin_b, "prefix:*")
                .unwrap(),
            1
        );
        assert!(cacache::SyncReader::open_hash(cache.base_dir(), shared_hash).is_err());
    }

    #[test]
    fn buffered_put_uses_the_same_per_object_quota_as_streaming() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        std::fs::create_dir_all(&base).unwrap();
        let index = Arc::new(MapIndex::default());
        let cache = ContentCache::new_with_limits(
            base,
            index.clone(),
            CacheLimits {
                max_object_bytes: 4,
                max_owner_partial_bytes: 8,
                max_owner_committed_bytes: 8,
                max_global_bytes: 16,
                min_free_space_bytes: 0,
                partial_ttl: std::time::Duration::from_secs(60),
                ..CacheLimits::default()
            },
        )
        .unwrap();

        let error = cache
            .put_for_owner(
                &CacheOwner::plugin("plugin-a"),
                "oversize",
                b"12345",
                CacheType::Other,
                None,
                None,
            )
            .unwrap_err();

        assert!(error.to_string().contains("per-object quota"));
        assert!(index.entries.lock().is_empty());
    }

    #[test]
    fn resource_config_drives_content_cache_limits() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let index = Arc::new(MapIndex::default());
        let mut config = crate::ResourceConfig::default();
        config.cache_limits.max_object_bytes = 3;
        config.cache_limits.min_free_space_bytes = 0;
        let cache = ContentCache::new_with_config(base, index, &config).unwrap();

        let error = cache
            .put("too-large", b"four", CacheType::Other, None, None)
            .unwrap_err();
        assert!(error.to_string().contains("per-object quota"));
    }

    #[test]
    fn queued_put_cannot_bypass_the_per_object_quota() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        std::fs::create_dir_all(&base).unwrap();
        let index = Arc::new(MapIndex::default());
        let cache = ContentCache::new_with_limits(
            base,
            index.clone(),
            CacheLimits {
                max_object_bytes: 4,
                max_owner_partial_bytes: 8,
                max_owner_committed_bytes: 8,
                max_global_bytes: 16,
                min_free_space_bytes: 0,
                partial_ttl: std::time::Duration::from_secs(60),
                ..CacheLimits::default()
            },
        )
        .unwrap();

        let error = cache
            .queue_put("oversize", b"12345".to_vec(), CacheType::Other, None, None)
            .unwrap_err();
        assert!(error.to_string().contains("per-object quota"));
        cache
            .queue_put("sentinel", b"x".to_vec(), CacheType::Other, None, None)
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !index
            .entries
            .lock()
            .contains_key(&CacheOwner::host().scoped_key("sentinel"))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "queued writer stalled"
            );
            std::thread::yield_now();
        }

        assert!(!index
            .entries
            .lock()
            .contains_key(&CacheOwner::host().scoped_key("oversize")));
    }

    #[test]
    fn queued_put_bounds_outstanding_item_count_and_releases_rejected_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(started_tx)),
            release_first_upsert: Mutex::new(Some(release_rx)),
        });
        let cache = ContentCache::new_with_limits(
            base.clone(),
            index.clone(),
            CacheLimits {
                max_queued_writes: 2,
                max_queued_bytes: 16,
                min_free_space_bytes: 0,
                ..CacheLimits::default()
            },
        )
        .unwrap();

        cache
            .queue_put("first", b"111".to_vec(), CacheType::Other, None, None)
            .unwrap();
        started_rx.recv().unwrap();
        cache
            .queue_put("second", b"222".to_vec(), CacheType::Other, None, None)
            .unwrap();
        let error = cache
            .queue_put("rejected", b"x".to_vec(), CacheType::Other, None, None)
            .unwrap_err();
        assert!(error.to_string().contains("queue item quota"));
        assert_eq!(
            std::fs::read_dir(base.join(".partial"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "reservation"))
                .count(),
            2
        );

        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !index
            .inner
            .entries
            .lock()
            .contains_key(&CacheOwner::host().scoped_key("second"))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "queued writer stalled"
            );
            std::thread::yield_now();
        }
        while std::fs::read_dir(base.join(".partial"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "reservation")
            })
        {
            assert!(
                std::time::Instant::now() < deadline,
                "queued writer did not release reservations"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn queued_put_bounds_total_buffered_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(started_tx)),
            release_first_upsert: Mutex::new(Some(release_rx)),
        });
        let cache = ContentCache::new_with_limits(
            base,
            index,
            CacheLimits {
                max_queued_writes: 8,
                max_queued_bytes: 4,
                min_free_space_bytes: 0,
                ..CacheLimits::default()
            },
        )
        .unwrap();

        cache
            .queue_put("first", b"111".to_vec(), CacheType::Other, None, None)
            .unwrap();
        started_rx.recv().unwrap();
        let error = cache
            .queue_put("rejected", b"22".to_vec(), CacheType::Other, None, None)
            .unwrap_err();
        assert!(error.to_string().contains("queue byte quota"));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn concurrent_same_key_replacements_do_not_leave_a_lost_writer_blob() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        std::fs::create_dir_all(&base).unwrap();
        let (second_upsert_entered, wait_for_second_upsert) = mpsc::channel();
        let index = Arc::new(RacingIndex {
            entry: Mutex::new(None),
            upsert_calls: AtomicUsize::new(0),
            second_upsert_entered,
            wait_for_second_upsert: Mutex::new(Some(wait_for_second_upsert)),
        });
        let cache = Arc::new(
            ContentCache::new_with_limits(
                base.clone(),
                index,
                CacheLimits {
                    min_free_space_bytes: 0,
                    ..CacheLimits::default()
                },
            )
            .unwrap(),
        );
        let start = Arc::new(Barrier::new(3));

        let mut workers = Vec::new();
        for body in [b"first".as_slice(), b"second".as_slice()] {
            let cache = cache.clone();
            let start = start.clone();
            workers.push(std::thread::spawn(move || {
                start.wait();
                cache.put("same", body, CacheType::Other, None, None)
            }));
        }
        start.wait();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }

        assert_eq!(count_content_files(&base.join("content-v2")), 1);
        assert!(cache.get("same").unwrap().is_some());
    }

    #[test]
    fn admission_reconciliation_does_not_delete_an_in_flight_different_key_blob() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let (first_upsert_started, first_upsert_started_rx) = mpsc::channel();
        let (release_first_upsert, release_first_upsert_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(first_upsert_started)),
            release_first_upsert: Mutex::new(Some(release_first_upsert_rx)),
        });
        let cache = Arc::new(
            ContentCache::new_with_limits(
                base,
                index,
                CacheLimits {
                    min_free_space_bytes: 0,
                    ..CacheLimits::default()
                },
            )
            .unwrap(),
        );

        let first_cache = cache.clone();
        let first = std::thread::spawn(move || {
            first_cache.put("first", b"first", CacheType::Other, None, None)
        });
        first_upsert_started_rx.recv().unwrap();

        let second_cache = cache.clone();
        let (second_finished, second_finished_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            let result = second_cache.put("second", b"second", CacheType::Other, None, None);
            second_finished.send(()).unwrap();
            result
        });
        assert!(
            second_finished_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "a different-key commit crossed an in-flight physical commit"
        );
        release_first_upsert.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert_eq!(
            cache.get("first").unwrap().as_deref(),
            Some(b"first".as_slice())
        );
        assert_eq!(
            cache.get("second").unwrap().as_deref(),
            Some(b"second".as_slice())
        );
    }

    #[test]
    fn streaming_commit_keeps_reconciliation_out_of_the_blob_index_window() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let (upsert_started, upsert_started_rx) = mpsc::channel();
        let (release_upsert, release_upsert_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(upsert_started)),
            release_first_upsert: Mutex::new(Some(release_upsert_rx)),
        });
        let limits = CacheLimits {
            min_free_space_bytes: 0,
            ..CacheLimits::default()
        };
        let cache = Arc::new(
            ContentCache::new_with_limits(base.clone(), index.clone(), limits.clone()).unwrap(),
        );

        let committing_cache = cache.clone();
        let commit = std::thread::spawn(move || {
            let key_lock = committing_cache
                .key_lock(&CacheOwner::host().scoped_key("stream"))
                .unwrap();
            let _key_guard = key_lock.lock();
            let mut writer = committing_cache.open_streaming_writer().unwrap();
            writer.write_all(b"streamed").unwrap();
            committing_cache.commit_streaming_for_owner_locked(
                &CacheOwner::host(),
                "stream",
                writer,
                CacheType::Other,
                None,
                None,
            )
        });
        upsert_started_rx.recv().unwrap();

        let (reconcile_finished, reconcile_finished_rx) = mpsc::channel();
        let reconcile_index = index.clone();
        let reconcile_base = base.clone();
        let reconcile = std::thread::spawn(move || {
            let result = ContentCache::new_with_limits(reconcile_base, reconcile_index, limits);
            reconcile_finished.send(()).unwrap();
            result
        });
        assert!(
            reconcile_finished_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "reconciliation crossed a committed blob before its index upsert"
        );

        release_upsert.send(()).unwrap();
        let (sri, bytes) = commit.join().unwrap().unwrap();
        reconcile.join().unwrap().unwrap();

        assert_eq!(bytes, 8);
        assert_eq!(
            cache.get("stream").unwrap().as_deref(),
            Some(b"streamed".as_slice())
        );
        assert!(
            cacache::SyncReader::open_hash(&base, sri.parse::<ssri::Integrity>().unwrap()).is_ok()
        );
    }

    fn assert_concurrent_different_key_puts_admit_one(limits: CacheLimits) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let (first_probe_started, first_probe_started_rx) = mpsc::channel();
        let (release_first_probe, release_first_probe_rx) = mpsc::channel();
        let index = Arc::new(AdmissionWindowIndex {
            inner: MapIndex::default(),
            first_probe_started: Mutex::new(Some(first_probe_started)),
            release_first_probe: Mutex::new(Some(release_first_probe_rx)),
        });
        let cache =
            Arc::new(ContentCache::new_with_limits(base.clone(), index.clone(), limits).unwrap());

        let first_cache = cache.clone();
        let first = std::thread::spawn(move || {
            first_cache.put("first", b"1111", CacheType::Other, None, None)
        });
        first_probe_started_rx.recv().unwrap();

        let second = cache.put("second", b"2222", CacheType::Other, None, None);
        release_first_probe.send(()).unwrap();
        let first = first.join().unwrap();

        assert!(first.is_ok());
        assert!(second.is_err());
        assert_eq!(index.inner.entries.lock().len(), 1);
        assert_eq!(count_content_files(&base.join("content-v2")), 1);
    }

    fn concurrent_admission_limits() -> CacheLimits {
        CacheLimits {
            max_object_bytes: 4,
            max_owner_partial_bytes: 16,
            max_owner_partial_objects: 4,
            max_owner_committed_bytes: 32,
            max_owner_committed_objects: 4,
            max_global_bytes: 32,
            max_global_partial_objects: 4,
            max_global_committed_objects: 4,
            min_free_space_bytes: 0,
            ..CacheLimits::default()
        }
    }

    #[test]
    fn concurrent_different_key_puts_cannot_over_admit_owner_bytes() {
        assert_concurrent_different_key_puts_admit_one(CacheLimits {
            max_owner_committed_bytes: 6,
            ..concurrent_admission_limits()
        });
    }

    #[test]
    fn concurrent_different_key_puts_cannot_over_admit_owner_objects() {
        assert_concurrent_different_key_puts_admit_one(CacheLimits {
            max_owner_committed_objects: 1,
            ..concurrent_admission_limits()
        });
    }

    #[test]
    fn concurrent_different_key_puts_cannot_over_admit_global_bytes() {
        assert_concurrent_different_key_puts_admit_one(CacheLimits {
            max_global_bytes: 6,
            ..concurrent_admission_limits()
        });
    }

    #[test]
    fn concurrent_different_key_puts_cannot_over_admit_global_objects() {
        assert_concurrent_different_key_puts_admit_one(CacheLimits {
            max_global_committed_objects: 1,
            ..concurrent_admission_limits()
        });
    }

    #[test]
    fn exact_delete_waits_for_same_key_put_and_removes_its_blob() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let owner = CacheOwner::plugin("delete-race");
        let (first_upsert_started, first_upsert_started_rx) = mpsc::channel();
        let (release_first_upsert, release_first_upsert_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(first_upsert_started)),
            release_first_upsert: Mutex::new(Some(release_first_upsert_rx)),
        });
        let cache = Arc::new(
            ContentCache::new_with_limits(
                base.clone(),
                index,
                CacheLimits {
                    min_free_space_bytes: 0,
                    ..CacheLimits::default()
                },
            )
            .unwrap(),
        );

        let put_cache = cache.clone();
        let put_owner = owner.clone();
        let put = std::thread::spawn(move || {
            put_cache.put_for_owner(&put_owner, "same", b"body", CacheType::Other, None, None)
        });
        first_upsert_started_rx.recv().unwrap();

        let delete_cache = cache.clone();
        let delete_owner = owner.clone();
        let (delete_finished, delete_finished_rx) = mpsc::channel();
        let delete = std::thread::spawn(move || {
            delete_finished
                .send(delete_cache.remove_for_owner(&delete_owner, "same"))
                .unwrap();
        });
        let completed_before_release = delete_finished_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .ok();
        let crossed_put = completed_before_release.is_some();
        release_first_upsert.send(()).unwrap();
        put.join().unwrap().unwrap();
        delete.join().unwrap();
        let removed = completed_before_release
            .unwrap_or_else(|| delete_finished_rx.recv().unwrap())
            .unwrap();

        assert!(!crossed_put, "same-key delete crossed the put mutation");
        assert!(removed);
        assert!(!cache.has_for_owner(&owner, "same").unwrap());
        assert_eq!(count_content_files(&base.join("content-v2")), 0);
    }

    #[test]
    fn pattern_delete_waits_for_put_and_removes_its_blob() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        let owner = CacheOwner::plugin("delete-race");
        let (first_upsert_started, first_upsert_started_rx) = mpsc::channel();
        let (release_first_upsert, release_first_upsert_rx) = mpsc::channel();
        let index = Arc::new(BlockingQueueIndex {
            inner: MapIndex::default(),
            first_upsert_started: Mutex::new(Some(first_upsert_started)),
            release_first_upsert: Mutex::new(Some(release_first_upsert_rx)),
        });
        let cache = Arc::new(
            ContentCache::new_with_limits(
                base.clone(),
                index,
                CacheLimits {
                    min_free_space_bytes: 0,
                    ..CacheLimits::default()
                },
            )
            .unwrap(),
        );

        let put_cache = cache.clone();
        let put_owner = owner.clone();
        let put = std::thread::spawn(move || {
            put_cache.put_for_owner(
                &put_owner,
                "prefix:item",
                b"body",
                CacheType::Other,
                None,
                None,
            )
        });
        first_upsert_started_rx.recv().unwrap();

        let delete_cache = cache.clone();
        let delete_owner = owner.clone();
        let (delete_finished, delete_finished_rx) = mpsc::channel();
        let delete = std::thread::spawn(move || {
            delete_finished
                .send(delete_cache.remove_by_pattern_for_owner(&delete_owner, "prefix:*"))
                .unwrap();
        });
        let completed_before_release = delete_finished_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .ok();
        let crossed_put = completed_before_release.is_some();
        release_first_upsert.send(()).unwrap();
        put.join().unwrap().unwrap();
        delete.join().unwrap();
        let removed = completed_before_release
            .unwrap_or_else(|| delete_finished_rx.recv().unwrap())
            .unwrap();

        assert!(
            !crossed_put,
            "pattern delete crossed the cache-root mutation"
        );
        assert_eq!(removed, 1);
        assert!(!cache.has_for_owner(&owner, "prefix:item").unwrap());
        assert_eq!(count_content_files(&base.join("content-v2")), 0);
    }

    #[test]
    fn construction_reconciles_unindexed_physical_blobs_and_corrupt_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        std::fs::create_dir_all(base.join(".partial")).unwrap();
        let orphan_sri = cacache::write_hash_sync(&base, b"crash orphan").unwrap();
        let corrupt_reservation = base.join(".partial").join("crash.reservation");
        let partial = base.join(".partial").join("crash.partial");
        let metadata = base.join(".partial").join("crash.meta");
        std::fs::write(&corrupt_reservation, b"not json").unwrap();
        std::fs::write(&partial, b"partial").unwrap();
        std::fs::write(&metadata, b"metadata").unwrap();

        assert!(cacache::SyncReader::open_hash(&base, orphan_sri.clone()).is_ok());
        let _cache = ContentCache::new(base.clone(), Arc::new(MapIndex::default())).unwrap();

        assert!(cacache::SyncReader::open_hash(&base, orphan_sri).is_err());
        assert!(!corrupt_reservation.exists());
        assert!(!partial.exists());
        assert!(!metadata.exists());
    }
}
