//! Streaming download → cacache pipeline with resume support.
//!
//! The plugin-side `fetch_to_cache` historically buffered the entire
//! response body in host RAM before writing into cacache. For large
//! blobs (chobit video downloads, ~1 GB+) that meant a corresponding
//! RAM spike, plus zero ability to resume after a network blip.
//!
//! This module funnels bytes through three stages without ever
//! materializing the whole body in memory:
//!
//! ```text
//! HTTP server ─stream chunks─▶ <cache>/.partial/<keyhash>.partial
//!                              (append + sidecar .meta with etag)
//!                                       │
//!                                  (on success)
//!                                       ▼
//!                            cacache content store (hash-only)
//!                                       │
//!                              (upsert key→SRI in SQLite index)
//!                                       │
//!                              (delete .partial + .meta)
//! ```
//!
//! Resume is allowed only when partial bytes are bound to the exact URL,
//! a strong ETag, and the expected resource total. Unbound or malformed
//! records are discarded before a request. A resumed response that does
//! not reproduce those validators is also discarded and retried once from
//! byte zero, so bytes from different representations are never combined.
//!
//! The network API publishes validated response identity before the first body
//! byte. The sidecar is installed atomically at that boundary, so an
//! interrupted strong-ETag response can resume without redownloading its
//! verified prefix.

use crate::features::content_cache::{CacheOwner, ContentCache};
use crate::shared::safe_log_fingerprint;
use anyhow::{Context, Result};
use arclain_db::CacheType;
use arclain_network::{AsyncHttpClient, StreamingDownload, StreamingResponseMetadata};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use tracing::{debug, info, warn};

use parking_lot::Mutex;

struct QuotaFile<'a> {
    file: &'a mut File,
    cache: &'a ContentCache,
    owner: &'a CacheOwner,
    scoped_key: &'a str,
    reservation_path: &'a Path,
    bytes_on_disk: u64,
    reservation_floor: &'a Cell<u64>,
    reserved_bytes: &'a Cell<u64>,
    quota_failed: &'a Cell<bool>,
}

impl Write for QuotaFile<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let required = u64::try_from(buf.len())
            .ok()
            .and_then(|length| self.bytes_on_disk.checked_add(length))
            .map(|requested| requested.max(self.reservation_floor.get()))
            .ok_or_else(|| {
                self.quota_failed.set(true);
                std::io::Error::other("cache per-object quota exceeded")
            })?;
        if required > self.reserved_bytes.get() {
            let requested = next_reservation_target(
                self.reserved_bytes.get(),
                required,
                self.cache.limits().reservation_chunk_bytes,
                self.cache.limits().max_object_bytes,
            )
            .ok_or_else(|| {
                self.quota_failed.set(true);
                std::io::Error::other("cache per-object quota exceeded")
            })?;
            if let Err(error) = self.cache.quota().reserve(
                self.cache.base_dir(),
                self.cache.cache_index(),
                self.owner,
                self.scoped_key,
                self.reservation_path,
                requested,
            ) {
                self.quota_failed.set(true);
                return Err(std::io::Error::other(error.to_string()));
            }
            self.reserved_bytes.set(requested);
        }

        match self.file.write(buf) {
            Ok(written) => {
                self.bytes_on_disk = self.bytes_on_disk.saturating_add(written as u64);
                Ok(written)
            }
            Err(error) => Err(error),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn next_reservation_target(
    current: u64,
    required: u64,
    configured_chunk: u64,
    max_object_bytes: u64,
) -> Option<u64> {
    if required > max_object_bytes {
        return None;
    }
    let chunk = configured_chunk.max(1);
    let rounded_required = required
        .checked_add(chunk.saturating_sub(1))?
        .checked_div(chunk)?
        .checked_mul(chunk)?;
    let geometric = if current == 0 {
        chunk
    } else {
        current.saturating_mul(2)
    };
    let target = rounded_required.max(geometric).min(max_object_bytes);
    (target >= required).then_some(target)
}

/// Sidecar metadata stored next to `<keyhash>.partial`.
///
/// URL, strong ETag, total size, and the current partial length form the
/// resume identity. Last-Modified is retained for diagnostics/compatibility,
/// but is not strong enough to authorize a resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartialMeta {
    #[serde(default)]
    requested_url: Option<String>,
    /// Final validated URL after redirects.
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    #[serde(default)]
    total_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResumeRecord {
    offset: u64,
    total_size: u64,
    etag: String,
    validated_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DownloadIdentity {
    cache_base_dir: PathBuf,
    cache_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadRequestDescriptor {
    url: String,
    cache_type: CacheType,
    product_id: Option<String>,
    use_proxy: bool,
    plugin_id: Option<String>,
}

impl DownloadRequestDescriptor {
    fn owner(&self) -> CacheOwner {
        self.plugin_id
            .as_deref()
            .map(CacheOwner::plugin)
            .unwrap_or_else(CacheOwner::host)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletedDownload {
    request: DownloadRequestDescriptor,
    bytes: u64,
}

#[derive(Debug)]
struct DownloadState {
    gate: Mutex<()>,
    completed: Mutex<Option<CompletedDownload>>,
    participants: AtomicUsize,
}

type DownloadLockRegistry = HashMap<DownloadIdentity, Weak<DownloadState>>;

static DOWNLOAD_LOCKS: OnceLock<Mutex<DownloadLockRegistry>> = OnceLock::new();

fn download_lock_registry() -> &'static Mutex<DownloadLockRegistry> {
    DOWNLOAD_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct DownloadIdentityLock {
    identity: DownloadIdentity,
    state: Arc<DownloadState>,
}

impl Drop for DownloadIdentityLock {
    fn drop(&mut self) {
        let previous_participants = self.state.participants.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous_participants > 0, "download participant underflow");
        if previous_participants != 1 {
            return;
        }

        let mut registry = download_lock_registry().lock();
        let current = registry.get(&self.identity);
        let points_to_state =
            current.is_some_and(|weak| Weak::ptr_eq(weak, &Arc::downgrade(&self.state)));
        if points_to_state && self.state.participants.load(Ordering::Acquire) == 0 {
            registry.remove(&self.identity);
        }
    }
}

fn download_identity_lock(cache_base_dir: &Path, key: &str) -> Result<DownloadIdentityLock> {
    let canonical_base = fs::canonicalize(cache_base_dir)
        .with_context(|| format!("canonicalizing cache directory {:?}", cache_base_dir))?;
    let identity = DownloadIdentity {
        cache_base_dir: canonical_base,
        cache_key: key.to_string(),
    };

    let mut registry = download_lock_registry().lock();
    registry.retain(|_, weak| weak.strong_count() > 0);
    let state = registry
        .get(&identity)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| {
            let state = Arc::new(DownloadState {
                gate: Mutex::new(()),
                completed: Mutex::new(None),
                participants: AtomicUsize::new(0),
            });
            registry.insert(identity.clone(), Arc::downgrade(&state));
            state
        });
    state.participants.fetch_add(1, Ordering::Relaxed);

    Ok(DownloadIdentityLock { identity, state })
}

/// Hash a cache key into a filesystem-safe sidecar name. cacache keys
/// can contain slashes, colons, etc. — sha256(key) gives us a flat
/// hex string with no escaping concerns.
fn key_to_sidecar_name(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    hex_encode(&digest)
}

/// Lowercase-hex encode a byte slice. Local helper to avoid pulling a
/// `hex` dep just for this.
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn partial_dir(cache: &ContentCache) -> PathBuf {
    cache.base_dir().join(".partial")
}

#[cfg(test)]
fn partial_paths(cache: &ContentCache, key: &str) -> (PathBuf, PathBuf) {
    partial_paths_for_owner(cache, &CacheOwner::host(), key)
}

fn partial_paths_for_owner(
    cache: &ContentCache,
    owner: &CacheOwner,
    key: &str,
) -> (PathBuf, PathBuf) {
    let dir = partial_dir(cache);
    let name = key_to_sidecar_name(&owner.scoped_key(key));
    let data = dir.join(format!("{}.partial", name));
    let meta = dir.join(format!("{}.meta", name));
    (data, meta)
}

fn read_meta(meta_path: &Path) -> Option<PartialMeta> {
    let json = fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&json).ok()
}

fn write_meta(meta_path: &Path, meta: &PartialMeta) -> Result<()> {
    let json = serde_json::to_string(meta).context("serializing partial meta")?;
    if let Some(parent) = meta_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating partial dir {:?}", parent))?;
    }
    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = meta_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("partial metadata path has no UTF-8 filename"))?;
    let temp_path = meta_path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));

    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("creating temporary partial meta {:?}", temp_path))?;
        file.write_all(json.as_bytes())
            .with_context(|| format!("writing temporary partial meta {:?}", temp_path))?;
        file.flush()
            .with_context(|| format!("flushing temporary partial meta {:?}", temp_path))?;
        file.sync_all()
            .with_context(|| format!("syncing temporary partial meta {:?}", temp_path))?;
        fs::rename(&temp_path, meta_path).with_context(|| {
            format!(
                "atomically installing partial meta {:?} from {:?}",
                meta_path, temp_path
            )
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn is_strong_etag(etag: &str) -> bool {
    let bytes = etag.as_bytes();
    bytes.len() >= 2
        && bytes.first() == Some(&b'"')
        && bytes.last() == Some(&b'"')
        && !etag.starts_with("W/")
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte) || *byte >= 0x80)
}

fn remove_record_file_with<R>(path: &Path, remove_file: &mut R) -> Result<()>
where
    R: FnMut(&Path) -> std::io::Result<()>,
{
    match remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing partial sidecar {:?}", path)),
    }
}

fn discard_partial_record_with<R>(
    data_path: &Path,
    meta_path: &Path,
    remove_file: &mut R,
) -> Result<()>
where
    R: FnMut(&Path) -> std::io::Result<()>,
{
    remove_record_file_with(meta_path, remove_file)?;
    remove_record_file_with(data_path, remove_file)?;
    Ok(())
}

#[cfg(test)]
fn prepare_resume_record(
    data_path: &Path,
    meta_path: &Path,
    requested_url: &str,
) -> Result<Option<ResumeRecord>> {
    prepare_resume_record_with(data_path, meta_path, requested_url, &mut |path| {
        fs::remove_file(path)
    })
}

fn prepare_resume_record_with<R>(
    data_path: &Path,
    meta_path: &Path,
    requested_url: &str,
    remove_file: &mut R,
) -> Result<Option<ResumeRecord>>
where
    R: FnMut(&Path) -> std::io::Result<()>,
{
    let data_exists = data_path
        .try_exists()
        .with_context(|| format!("checking partial data {:?}", data_path))?;
    let meta_exists = meta_path
        .try_exists()
        .with_context(|| format!("checking partial metadata {:?}", meta_path))?;
    if !data_exists && !meta_exists {
        return Ok(None);
    }

    let candidate = (|| {
        let offset = fs::metadata(data_path).ok()?.len();
        let meta = read_meta(meta_path)?;
        let total_size = meta.total_size?;
        let etag = meta.etag?;
        if meta.requested_url.as_deref() != Some(requested_url)
            || !is_strong_etag(&etag)
            || offset == 0
            || offset >= total_size
        {
            return None;
        }
        Some(ResumeRecord {
            offset,
            total_size,
            etag,
            validated_url: meta.url,
        })
    })();

    if candidate.is_none() {
        discard_partial_record_with(data_path, meta_path, remove_file)?;
    }
    Ok(candidate)
}

/// Stream-fetch `url` and store the response under `key` in `cache`.
///
/// Uses a `.partial` intermediate so a failed attempt can resume on
/// the next call (HTTP-Range based, ETag-validated). On success, the
/// partial file is collapsed into the cacache content store (single
/// hash-addressed blob) and the key → SRI mapping is upserted into
/// the cache's SQLite index.
///
/// Returns the number of bytes ultimately written to cacache.
///
/// Synchronous — must be called from a background thread; never from
/// the UI thread. The underlying `AsyncHttpClient` uses `block_on`.
pub fn fetch_url_to_cache(
    cache: &ContentCache,
    http_client: &AsyncHttpClient,
    key: &str,
    url: &str,
    cache_type: CacheType,
    product_id: Option<&str>,
    use_proxy: bool,
) -> Result<u64, String> {
    fetch_url_to_cache_with_metadata(
        cache,
        key,
        url,
        cache_type,
        product_id,
        use_proxy,
        |writer, range_start, if_match, bind_metadata| {
            http_client.blocking_get_streaming_with_metadata(
                url,
                use_proxy,
                writer,
                range_start,
                if_match,
                |metadata| bind_metadata(metadata),
            )
        },
    )
}

/// Checked plugin variant of [`fetch_url_to_cache`]. No URL controlled by a
/// plugin is ever sent through a host-only client, including redirects and
/// streaming retries.
pub fn fetch_url_to_cache_for_plugin(
    cache: &ContentCache,
    http_client: &AsyncHttpClient,
    key: &str,
    url: &str,
    cache_type: CacheType,
    product_id: Option<&str>,
    plugin_id: &str,
) -> Result<u64, String> {
    let request = DownloadRequestDescriptor {
        url: url.to_string(),
        cache_type,
        product_id: product_id.map(str::to_string),
        use_proxy: false,
        plugin_id: Some(plugin_id.to_string()),
    };
    fetch_url_to_cache_with_metadata_file_ops(
        cache,
        key,
        request,
        |writer, range_start, if_match, bind_metadata| {
            http_client
                .blocking_get_streaming_for_plugin_with_metadata(
                    plugin_id,
                    url,
                    writer,
                    range_start,
                    if_match,
                    |metadata| bind_metadata(metadata),
                )
                .map_err(|error| error.to_string())
        },
        |path| fs::remove_file(path),
        false,
    )
}

fn fetch_url_to_cache_with_metadata<F>(
    cache: &ContentCache,
    key: &str,
    url: &str,
    cache_type: CacheType,
    product_id: Option<&str>,
    use_proxy: bool,
    fetch: F,
) -> Result<u64, String>
where
    F: FnMut(
        &mut QuotaFile<'_>,
        Option<u64>,
        Option<&str>,
        &mut dyn FnMut(&StreamingResponseMetadata) -> Result<(), String>,
    ) -> Result<StreamingDownload, String>,
{
    let request = DownloadRequestDescriptor {
        url: url.to_string(),
        cache_type,
        product_id: product_id.map(str::to_string),
        use_proxy,
        plugin_id: None,
    };
    fetch_url_to_cache_with_metadata_file_ops(
        cache,
        key,
        request,
        fetch,
        |path| fs::remove_file(path),
        false,
    )
}

#[cfg(test)]
fn fetch_url_to_cache_with<F>(
    cache: &ContentCache,
    key: &str,
    url: &str,
    cache_type: CacheType,
    product_id: Option<&str>,
    use_proxy: bool,
    fetch: F,
) -> Result<u64, String>
where
    F: FnMut(&mut QuotaFile<'_>, Option<u64>, Option<&str>) -> Result<StreamingDownload, String>,
{
    let request = DownloadRequestDescriptor {
        url: url.to_string(),
        cache_type,
        product_id: product_id.map(str::to_string),
        use_proxy,
        plugin_id: None,
    };
    fetch_url_to_cache_with_file_ops(cache, key, request, fetch, |path| fs::remove_file(path))
}

#[cfg(test)]
fn fetch_url_to_cache_with_file_ops<F, R>(
    cache: &ContentCache,
    key: &str,
    request_descriptor: DownloadRequestDescriptor,
    mut fetch: F,
    remove_file: R,
) -> Result<u64, String>
where
    F: FnMut(&mut QuotaFile<'_>, Option<u64>, Option<&str>) -> Result<StreamingDownload, String>,
    R: FnMut(&Path) -> std::io::Result<()>,
{
    let requested_url = request_descriptor.url.clone();
    fetch_url_to_cache_with_metadata_file_ops(
        cache,
        key,
        request_descriptor,
        move |writer, range_start, if_match, bind_metadata| {
            let result = fetch(writer, range_start, if_match)?;
            bind_metadata(&StreamingResponseMetadata {
                validated_url: requested_url.clone(),
                was_partial: result.was_partial,
                etag: result.etag.clone(),
                last_modified: result.last_modified.clone(),
                range_start: result.was_partial.then_some(range_start).flatten(),
                total_size: result.total_size,
                expected_body_length: Some(result.bytes_written),
            })?;
            Ok(result)
        },
        remove_file,
        true,
    )
}

fn bind_response_metadata(
    meta_path: &Path,
    requested_url: &str,
    resume: Option<&ResumeRecord>,
    metadata: &StreamingResponseMetadata,
) -> Result<(), String> {
    if let Some(record) = resume {
        if !metadata.was_partial
            || metadata.range_start != Some(record.offset)
            || metadata.total_size != Some(record.total_size)
            || metadata.etag.as_deref() != Some(record.etag.as_str())
            || !metadata.etag.as_deref().is_some_and(is_strong_etag)
            || metadata.validated_url != record.validated_url
        {
            return Err(
                "resume response metadata does not match the bound representation".to_string(),
            );
        }
    } else if metadata.was_partial {
        return Err("fresh streaming request unexpectedly returned partial content".to_string());
    }

    write_meta(
        meta_path,
        &PartialMeta {
            requested_url: Some(requested_url.to_string()),
            url: metadata.validated_url.clone(),
            etag: metadata.etag.clone(),
            last_modified: metadata.last_modified.clone(),
            total_size: metadata.total_size,
        },
    )
    .map_err(|error| format!("binding partial response metadata: {error:#}"))
}

fn execute_fetch_attempt<F>(
    fetch: &mut F,
    file: &mut File,
    range_start: Option<u64>,
    if_match: Option<&str>,
    meta_path: &Path,
    requested_url: &str,
    resume: Option<&ResumeRecord>,
    cache: &ContentCache,
    owner: &CacheOwner,
    scoped_key: &str,
    reservation_path: &Path,
) -> (Result<StreamingDownload, String>, bool)
where
    F: FnMut(
        &mut QuotaFile<'_>,
        Option<u64>,
        Option<&str>,
        &mut dyn FnMut(&StreamingResponseMetadata) -> Result<(), String>,
    ) -> Result<StreamingDownload, String>,
{
    let quota_failed = Cell::new(false);
    let reservation_floor = Cell::new(0_u64);
    let initial_bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let reserved_bytes = Cell::new(initial_bytes);
    let mut callback_count = 0_usize;
    let mut bind_metadata = |metadata: &StreamingResponseMetadata| {
        callback_count += 1;
        if callback_count != 1 {
            return Err("streaming metadata callback ran more than once".to_string());
        }
        if let Some(total_size) = metadata.total_size {
            if total_size != reserved_bytes.get() {
                if let Err(error) = cache.quota().reserve(
                    cache.base_dir(),
                    cache.cache_index(),
                    owner,
                    scoped_key,
                    reservation_path,
                    total_size,
                ) {
                    quota_failed.set(true);
                    return Err(error.to_string());
                }
                reserved_bytes.set(total_size);
            }
            reservation_floor.set(total_size);
        }
        bind_response_metadata(meta_path, requested_url, resume, metadata)
    };
    let mut quota_file = QuotaFile {
        file,
        cache,
        owner,
        scoped_key,
        reservation_path,
        bytes_on_disk: initial_bytes,
        reservation_floor: &reservation_floor,
        reserved_bytes: &reserved_bytes,
        quota_failed: &quota_failed,
    };
    let result = fetch(&mut quota_file, range_start, if_match, &mut bind_metadata);
    let result = match result {
        Ok(result) if callback_count == 1 => Ok(result),
        Ok(_) => Err("streaming transport completed without response metadata".to_string()),
        Err(error) => Err(error),
    };
    (result, quota_failed.get())
}

fn fetch_url_to_cache_with_metadata_file_ops<F, R>(
    cache: &ContentCache,
    key: &str,
    request_descriptor: DownloadRequestDescriptor,
    mut fetch: F,
    mut remove_file: R,
    retry_resume_errors: bool,
) -> Result<u64, String>
where
    F: FnMut(
        &mut QuotaFile<'_>,
        Option<u64>,
        Option<&str>,
        &mut dyn FnMut(&StreamingResponseMetadata) -> Result<(), String>,
    ) -> Result<StreamingDownload, String>,
    R: FnMut(&Path) -> std::io::Result<()>,
{
    let owner = request_descriptor.owner();
    let scoped_key = owner.scoped_key(key);
    let (partial_path, meta_path) = partial_paths_for_owner(cache, &owner, key);
    let reservation_path = partial_path.with_extension("reservation");
    fs::create_dir_all(partial_dir(cache).as_path())
        .map_err(|e| format!("creating partial dir: {}", e))?;

    let identity_lock = download_identity_lock(cache.base_dir(), &scoped_key)
        .map_err(|error| format!("locking streaming download identity: {error:#}"))?;
    let _identity_guard = identity_lock.state.gate.lock();
    let cache_key_lock = cache
        .key_lock(&scoped_key)
        .map_err(|error| format!("locking scoped cache key: {error:#}"))?;
    let _cache_key_guard = cache_key_lock.lock();
    // Callers that overlapped the successful owner share its committed
    // result. Once the last participant drops, the weak registry entry and
    // this outcome disappear, so a later independent call still refetches.
    if let Some(completed) = identity_lock.state.completed.lock().as_ref() {
        if completed.request == request_descriptor {
            return Ok(completed.bytes);
        }
    }

    let resume = prepare_resume_record_with(
        &partial_path,
        &meta_path,
        &request_descriptor.url,
        &mut remove_file,
    )
    .map_err(|error| format!("validating partial download: {error:#}"))?;

    if let Err(error) = cache.quota().reserve(
        cache.base_dir(),
        cache.cache_index(),
        &owner,
        &scoped_key,
        &reservation_path,
        resume.as_ref().map_or(0, |record| record.offset),
    ) {
        let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
        let _ = cache.quota().release(&reservation_path);
        return Err(error.to_string());
    }

    if let Some(record) = resume.as_ref() {
        remove_record_file_with(&meta_path, &mut remove_file)
            .map_err(|error| format!("revoking resume metadata before append: {error:#}"))?;
        info!(
            "[streaming] resuming {} from byte {} (etag: {})",
            safe_log_fingerprint(key),
            record.offset,
            safe_log_fingerprint(&record.etag)
        );
    }

    let mut file = if resume.is_some() {
        OpenOptions::new()
            .append(true)
            .open(&partial_path)
            .map_err(|e| format!("opening resumable partial file: {e}"))?
    } else {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&partial_path)
            .map_err(|e| format!("opening fresh partial file: {e}"))?
    };

    let initial_result = execute_fetch_attempt(
        &mut fetch,
        &mut file,
        resume.as_ref().map(|record| record.offset),
        resume.as_ref().map(|record| record.etag.as_str()),
        &meta_path,
        &request_descriptor.url,
        resume.as_ref(),
        cache,
        &owner,
        &scoped_key,
        &reservation_path,
    );
    let mut did_restart = false;
    let (initial_result, initial_quota_failed) = initial_result;
    let mut result = match initial_result {
        Ok(result) => result,
        Err(error) if resume.is_some() && retry_resume_errors && !initial_quota_failed => {
            warn!(
                "[streaming] resume stream failed for {}; discarding appended bytes before restart",
                safe_log_fingerprint(key)
            );
            drop(file);
            discard_partial_record_with(&partial_path, &meta_path, &mut remove_file).map_err(
                |discard_error| {
                    format!(
                        "discarding partial download after resume error ({error}): {discard_error:#}"
                    )
                },
            )?;
            cache
                .quota()
                .release(&reservation_path)
                .map_err(|release_error| format!("releasing failed resume: {release_error:#}"))?;
            cache
                .quota()
                .reserve(
                    cache.base_dir(),
                    cache.cache_index(),
                    &owner,
                    &scoped_key,
                    &reservation_path,
                    0,
                )
                .map_err(|reserve_error| {
                    format!("reserving clean restart capacity: {reserve_error:#}")
                })?;
            file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&partial_path)
                .map_err(|open_error| format!("opening clean restart file: {open_error}"))?;
            did_restart = true;
            let (restart_result, restart_quota_failed) = execute_fetch_attempt(
                &mut fetch,
                &mut file,
                None,
                None,
                &meta_path,
                &request_descriptor.url,
                None,
                cache,
                &owner,
                &scoped_key,
                &reservation_path,
            );
            match restart_result {
                Ok(result) => result,
                Err(restart_error) => {
                    drop(file);
                    if restart_quota_failed {
                        let _ = discard_partial_record_with(
                            &partial_path,
                            &meta_path,
                            &mut remove_file,
                        );
                        let _ = cache.quota().release(&reservation_path);
                    } else if let Ok(metadata) = fs::metadata(&partial_path) {
                        let _ = cache.quota().reserve(
                            cache.base_dir(),
                            cache.cache_index(),
                            &owner,
                            &scoped_key,
                            &reservation_path,
                            metadata.len(),
                        );
                    }
                    return Err(format!(
                        "resume failed ({error}); clean restart failed: {restart_error}"
                    ));
                }
            }
        }
        Err(error) => {
            drop(file);
            if initial_quota_failed {
                let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
                let _ = cache.quota().release(&reservation_path);
            } else if let Ok(metadata) = fs::metadata(&partial_path) {
                let _ = cache.quota().reserve(
                    cache.base_dir(),
                    cache.cache_index(),
                    &owner,
                    &scoped_key,
                    &reservation_path,
                    metadata.len(),
                );
            }
            return Err(error);
        }
    };

    let resume_was_validated = !did_restart
        && resume.as_ref().is_some_and(|record| {
            result.was_partial
                && result.total_size == Some(record.total_size)
                && result.etag.as_deref() == Some(record.etag.as_str())
                && result.etag.as_deref().is_some_and(is_strong_etag)
        });

    if resume.is_some() && !did_restart && !resume_was_validated {
        warn!(
            "[streaming] resume response did not match bound metadata for {}; restarting",
            safe_log_fingerprint(key)
        );
        drop(file);
        discard_partial_record_with(&partial_path, &meta_path, &mut remove_file)
            .map_err(|error| format!("discarding mismatched partial download: {error:#}"))?;
        file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&partial_path)
            .map_err(|e| format!("opening clean restart file: {e}"))?;
        cache
            .quota()
            .release(&reservation_path)
            .map_err(|error| format!("releasing mismatched resume reservation: {error:#}"))?;
        cache
            .quota()
            .reserve(
                cache.base_dir(),
                cache.cache_index(),
                &owner,
                &scoped_key,
                &reservation_path,
                0,
            )
            .map_err(|error| format!("reserving clean restart capacity: {error:#}"))?;
        let (restart_result, restart_quota_failed) = execute_fetch_attempt(
            &mut fetch,
            &mut file,
            None,
            None,
            &meta_path,
            &request_descriptor.url,
            None,
            cache,
            &owner,
            &scoped_key,
            &reservation_path,
        );
        result = match restart_result {
            Ok(result) => result,
            Err(error) => {
                drop(file);
                if restart_quota_failed {
                    let _ =
                        discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
                    let _ = cache.quota().release(&reservation_path);
                }
                return Err(error);
            }
        };
        did_restart = true;
    }
    if (resume.is_none() || did_restart) && result.was_partial {
        drop(file);
        let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
        let _ = cache.quota().release(&reservation_path);
        return Err("fresh streaming request unexpectedly returned partial content".to_string());
    }

    file.flush()
        .map_err(|error| format!("flushing completed partial file: {error}"))?;

    let partial_size = fs::metadata(&partial_path)
        .map_err(|error| format!("reading completed partial size: {error}"))?
        .len();
    let total_size = resume
        .as_ref()
        .filter(|_| resume_was_validated)
        .map(|record| record.total_size)
        .unwrap_or_else(|| result.total_size.unwrap_or(partial_size));
    if partial_size != total_size {
        let _ = cache.quota().reserve(
            cache.base_dir(),
            cache.cache_index(),
            &owner,
            &scoped_key,
            &reservation_path,
            partial_size,
        );
        return Err(format!(
            "completed partial size {partial_size} does not match resource total {total_size}"
        ));
    }

    drop(file);

    let commit_admission = match cache.quota().prepare_commit(
        cache.base_dir(),
        cache.cache_index(),
        &owner,
        &scoped_key,
        partial_size,
    ) {
        Ok(admission) => admission,
        Err(error) => {
            let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
            let _ = cache.quota().release(&reservation_path);
            return Err(error.to_string());
        }
    };

    // Collapse the .partial file into cacache. We stream the partial's
    // contents through cacache::SyncWriter so we don't load it into
    // memory; this is the second pass over the bytes (the first being
    // the network → .partial pass).
    let mut reader = File::open(&partial_path)
        .map_err(|e| format!("re-opening partial for cacache transfer: {}", e))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| format!("seeking partial: {}", e))?;
    let mut writer = cache
        .open_streaming_writer()
        .map_err(|e| format!("opening cache writer: {}", e))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("reading partial: {}", e))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("writing to cache: {}", e))?;
    }
    let (sri, bytes_committed) = match cache.commit_streaming_for_owner_locked(
        &owner,
        key,
        writer,
        request_descriptor.cache_type,
        request_descriptor.product_id.as_deref(),
        Some(&request_descriptor.url),
    ) {
        Ok(committed) => committed,
        Err(error) => {
            let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
            let _ = cache.quota().release(&reservation_path);
            return Err(format!("committing cache blob and index: {error}"));
        }
    };
    drop(commit_admission);

    // Cleanup partial sidecars on success. Best-effort — orphans get
    // GC'd by `.partial` directory cleanup on next run if removal
    // fails (e.g. on Windows when antivirus is mid-scan).
    let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);
    cache
        .quota()
        .release(&reservation_path)
        .map_err(|error| format!("releasing completed cache reservation: {error:#}"))?;

    debug!(
        "[streaming] cached {} bytes for key {} (partial was {}, sri {})",
        bytes_committed,
        safe_log_fingerprint(key),
        partial_size,
        safe_log_fingerprint(&sri)
    );
    *identity_lock.state.completed.lock() = Some(CompletedDownload {
        request: request_descriptor,
        bytes: bytes_committed,
    });
    Ok(bytes_committed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::content_cache::{CacheLimits, CacheOwner};
    use crate::traits::CacheIndex;
    use arclain_db::CacheEntry;
    use arclain_network::features::proxy::ProxyConfig;
    use arclain_network::{PluginNetworkPolicy, StreamingDownload, StreamingResponseMetadata};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct RecordingIndex {
        entry: Mutex<Option<CacheEntry>>,
        upserts: AtomicUsize,
        first_upsert_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        first_upsert_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    struct FailingCommitIndex;

    impl CacheIndex for FailingCommitIndex {
        fn upsert(
            &self,
            _key: &str,
            _product_id: Option<&str>,
            _content_hash: &str,
            _source_url: Option<&str>,
            _cache_type: CacheType,
            _size_bytes: Option<i64>,
        ) -> Result<i64> {
            anyhow::bail!("injected index failure")
        }

        fn get(&self, _key: &str) -> Result<Option<CacheEntry>> {
            Ok(None)
        }
        fn has(&self, _key: &str) -> Result<bool> {
            Ok(false)
        }
        fn delete(&self, _key: &str) -> Result<bool> {
            Ok(false)
        }
        fn delete_by_pattern(&self, _pattern: &str) -> Result<usize> {
            Ok(0)
        }
        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }

        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    impl CacheIndex for RecordingIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            if let Some(started) = self.first_upsert_started.lock().take() {
                started.send(()).unwrap();
                self.first_upsert_release
                    .lock()
                    .take()
                    .expect("blocking upsert release receiver")
                    .recv()
                    .unwrap();
            }
            self.upserts.fetch_add(1, Ordering::SeqCst);
            *self.entry.lock() = Some(CacheEntry {
                id: 1,
                key: key.to_string(),
                product_id: product_id.map(str::to_string),
                content_hash: content_hash.to_string(),
                source_url: source_url.map(str::to_string),
                cache_type,
                created_at: String::new(),
                last_accessed: None,
                size_bytes,
            });
            Ok(1)
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

        fn delete(&self, _key: &str) -> Result<bool> {
            Ok(false)
        }

        fn delete_by_pattern(&self, _pattern: &str) -> Result<usize> {
            Ok(0)
        }

        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }
    }

    fn test_cache() -> (tempfile::TempDir, ContentCache, Arc<RecordingIndex>) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        fs::create_dir_all(&base).unwrap();
        let index = Arc::new(RecordingIndex::default());
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

    fn test_cache_with_limits(
        mut limits: CacheLimits,
    ) -> (tempfile::TempDir, ContentCache, Arc<RecordingIndex>) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        fs::create_dir_all(&base).unwrap();
        let index = Arc::new(RecordingIndex::default());
        limits.min_free_space_bytes = 0;
        let cache = ContentCache::new_with_limits(base, index.clone(), limits).unwrap();
        (dir, cache, index)
    }

    fn streaming_result(
        bytes_written: u64,
        was_partial: bool,
        etag: Option<&str>,
        total_size: Option<u64>,
    ) -> StreamingDownload {
        StreamingDownload {
            bytes_written,
            was_partial,
            etag: etag.map(str::to_string),
            last_modified: None,
            total_size,
        }
    }

    fn read_socks_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set SOCKS test connection timeout");

        let mut greeting = [0_u8; 2];
        stream
            .read_exact(&mut greeting)
            .expect("read SOCKS5 greeting prefix");
        assert_eq!(greeting[0], 0x05);
        let mut methods = vec![0_u8; usize::from(greeting[1])];
        stream
            .read_exact(&mut methods)
            .expect("read SOCKS5 authentication methods");
        assert!(
            methods.contains(&0x00),
            "client did not offer no-auth SOCKS5"
        );
        stream
            .write_all(&[0x05, 0x00])
            .expect("select SOCKS5 no-authentication");

        let mut connect = [0_u8; 4];
        stream
            .read_exact(&mut connect)
            .expect("read SOCKS5 CONNECT prefix");
        assert_eq!(&connect[..3], &[0x05, 0x01, 0x00]);
        assert_eq!(connect[3], 0x01, "checked target was not pinned as IPv4");
        let mut target = [0_u8; 4];
        stream
            .read_exact(&mut target)
            .expect("read pinned SOCKS5 target");
        assert_eq!(target, [1, 1, 1, 1], "proxy observed the wrong target");
        let mut port = [0_u8; 2];
        stream
            .read_exact(&mut port)
            .expect("read pinned SOCKS5 target port");
        assert_eq!(u16::from_be_bytes(port), 80);
        stream
            .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
            .expect("acknowledge SOCKS5 CONNECT");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read HTTP request");
            assert!(read > 0, "HTTP request closed before its headers completed");
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(request).expect("checked HTTP request was not ASCII")
    }

    fn checked_plugin_client(
        runtime: &tokio::runtime::Runtime,
        proxy_address: Option<std::net::SocketAddr>,
        plugin_id: &str,
        network_enabled: bool,
    ) -> AsyncHttpClient {
        let whitelist = Arc::new(parking_lot::RwLock::new(
            arclain_network::DomainWhitelist::default(),
        ));
        let proxy = proxy_address.map(|address| ProxyConfig {
            enabled: true,
            address: address.to_string(),
            username: None,
            password: None,
        });
        let client = AsyncHttpClient::new(runtime.handle().clone(), whitelist, proxy);
        client.configure_plugin(
            plugin_id,
            PluginNetworkPolicy {
                network_enabled,
                requests_per_minute: 4,
            },
        );
        client.replace_plugin_manifest_domains(plugin_id, &["1.1.1.1".to_string()]);
        if proxy_address.is_some() {
            client.apply_plugin_proxy_map(HashMap::from([(plugin_id.to_string(), true)]));
        }
        client
    }

    #[test]
    fn key_hash_is_stable_and_distinct() {
        let a = key_to_sidecar_name("dlsite:RJ12345:video:abc:480p");
        let b = key_to_sidecar_name("dlsite:RJ12345:video:abc:720p");
        assert_eq!(a.len(), 64, "sha256 hex is 64 chars");
        assert_ne!(a, b, "different keys → different hashes");
        // Stable across calls
        assert_eq!(a, key_to_sidecar_name("dlsite:RJ12345:video:abc:480p"));
    }

    #[test]
    fn key_hash_is_filesystem_safe() {
        let h = key_to_sidecar_name("contains/slashes:and:colons");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn write_and_read_meta_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.meta");
        let meta = PartialMeta {
            requested_url: Some("https://example.com/v.mp4".to_string()),
            url: "https://example.com/v.mp4".to_string(),
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string()),
            total_size: Some(4096),
        };
        write_meta(&path, &meta).unwrap();
        let read = read_meta(&path).unwrap();
        assert_eq!(read.url, meta.url);
        assert_eq!(read.etag, meta.etag);
        assert_eq!(read.last_modified, meta.last_modified);
        assert_eq!(read.total_size, meta.total_size);
    }

    #[test]
    fn read_missing_meta_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.meta");
        assert!(read_meta(&path).is_none());
    }

    fn four_byte_object_limits() -> CacheLimits {
        CacheLimits {
            max_object_bytes: 4,
            max_owner_partial_bytes: 8,
            max_owner_committed_bytes: 8,
            max_global_bytes: 16,
            partial_ttl: Duration::from_secs(60),
            ..CacheLimits::default()
        }
    }

    #[test]
    fn declared_oversize_is_rejected_before_body_and_cleans_all_sidecars() {
        let (_dir, cache, index) = test_cache_with_limits(four_byte_object_limits());
        let key = "declared-oversize";
        let url = "https://example.test/declared";
        let (partial_path, meta_path) = partial_paths(&cache, key);
        let reservation_path = partial_path.with_extension("reservation");
        let body_writes = Arc::new(AtomicUsize::new(0));
        let writes = body_writes.clone();

        let error = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            move |writer, _range_start, _if_match, bind_metadata| {
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: url.to_string(),
                    was_partial: false,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: Some(5),
                    expected_body_length: Some(5),
                })?;
                writes.fetch_add(1, Ordering::SeqCst);
                writer
                    .write_all(b"12345")
                    .map_err(|error| error.to_string())?;
                Ok(streaming_result(5, false, Some("\"v1\""), Some(5)))
            },
        )
        .unwrap_err();

        assert!(error.contains("per-object quota"));
        assert_eq!(body_writes.load(Ordering::SeqCst), 0);
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
        assert!(!partial_path.exists());
        assert!(!meta_path.exists());
        assert!(!reservation_path.exists());
    }

    #[test]
    fn chunked_oversize_is_stopped_during_write_and_cleans_all_sidecars() {
        let (_dir, cache, index) = test_cache_with_limits(four_byte_object_limits());
        let key = "chunked-oversize";
        let url = "https://example.test/chunked";
        let (partial_path, meta_path) = partial_paths(&cache, key);
        let reservation_path = partial_path.with_extension("reservation");

        let error = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            move |writer, _range_start, _if_match, bind_metadata| {
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: url.to_string(),
                    was_partial: false,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: None,
                    expected_body_length: None,
                })?;
                writer
                    .write_all(b"12345")
                    .map_err(|error| error.to_string())?;
                Ok(streaming_result(5, false, Some("\"v1\""), None))
            },
        )
        .unwrap_err();

        assert!(error.contains("per-object quota"));
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
        assert!(!partial_path.exists());
        assert!(!meta_path.exists());
        assert!(!reservation_path.exists());
    }

    #[test]
    fn unknown_length_stream_amortizes_persistent_reservation_updates() {
        let limits = CacheLimits {
            max_object_bytes: 8 * 1024,
            max_owner_partial_bytes: 16 * 1024,
            max_owner_committed_bytes: 16 * 1024,
            max_global_bytes: 32 * 1024,
            reservation_chunk_bytes: 256,
            min_free_space_bytes: 0,
            ..CacheLimits::default()
        };
        let (_dir, cache, _index) = test_cache_with_limits(limits);
        let writes_before = cache.quota().reservation_write_count();

        fetch_url_to_cache_with_metadata(
            &cache,
            "amortized",
            "https://example.test/amortized",
            CacheType::Other,
            None,
            false,
            |writer, _range_start, _if_match, bind_metadata| {
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: "https://example.test/amortized".to_string(),
                    was_partial: false,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: None,
                    expected_body_length: None,
                })?;
                for _ in 0..4096 {
                    writer.write_all(b"x").map_err(|error| error.to_string())?;
                }
                Ok(streaming_result(4096, false, Some("\"v1\""), None))
            },
        )
        .unwrap();

        let reservation_writes = cache.quota().reservation_write_count() - writes_before;
        assert!(
            reservation_writes <= 16,
            "reservation persistence was not amortized: {reservation_writes} writes"
        );
    }

    #[test]
    fn concurrent_declared_streams_keep_full_aggregate_reservations_while_writing() {
        let limits = CacheLimits {
            max_object_bytes: 8,
            max_owner_partial_bytes: 10,
            max_owner_committed_bytes: 32,
            max_global_bytes: 64,
            partial_ttl: Duration::from_secs(60),
            ..CacheLimits::default()
        };
        let (_dir, cache, _index) = test_cache_with_limits(limits);
        let (first_reserved_tx, first_reserved_rx) = std::sync::mpsc::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();
        let first_cache = cache.clone();
        let first = std::thread::spawn(move || {
            fetch_url_to_cache_with_metadata(
                &first_cache,
                "first",
                "https://example.test/first",
                CacheType::Other,
                None,
                false,
                |writer, _range_start, _if_match, bind_metadata| {
                    bind_metadata(&StreamingResponseMetadata {
                        validated_url: "https://example.test/first".to_string(),
                        was_partial: false,
                        etag: Some("\"first\"".to_string()),
                        last_modified: None,
                        range_start: None,
                        total_size: Some(6),
                        expected_body_length: Some(6),
                    })?;
                    writer.write_all(b"1").unwrap();
                    first_reserved_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    Ok(streaming_result(1, false, Some("\"first\""), Some(6)))
                },
            )
        });
        first_reserved_rx.recv().unwrap();

        let second = fetch_url_to_cache_with_metadata(
            &cache,
            "second",
            "https://example.test/second",
            CacheType::Other,
            None,
            false,
            |writer, _range_start, _if_match, bind_metadata| {
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: "https://example.test/second".to_string(),
                    was_partial: false,
                    etag: Some("\"second\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: Some(5),
                    expected_body_length: Some(5),
                })?;
                writer.write_all(b"12345").unwrap();
                Ok(streaming_result(5, false, Some("\"second\""), Some(5)))
            },
        );
        release_first_tx.send(()).unwrap();
        let _ = first.join().unwrap();

        assert!(second
            .expect_err("aggregate reservation should reject the second stream")
            .contains("owner partial quota"));
    }

    #[test]
    fn index_commit_failure_releases_reservation_partial_and_unreferenced_blob() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        fs::create_dir_all(&base).unwrap();
        let cache = ContentCache::new_with_limits(
            base.clone(),
            Arc::new(FailingCommitIndex),
            CacheLimits {
                min_free_space_bytes: 0,
                ..CacheLimits::default()
            },
        )
        .unwrap();
        let key = "commit-failure";
        let url = "https://example.test/failure";
        let (partial_path, meta_path) = partial_paths(&cache, key);
        let reservation_path = partial_path.with_extension("reservation");

        let error = fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, _range_start, _if_match| {
                writer.write_all(b"abc").unwrap();
                Ok(streaming_result(3, false, Some("\"v1\""), Some(3)))
            },
        )
        .unwrap_err();

        assert!(error.contains("injected index failure"));
        assert!(!partial_path.exists());
        assert!(!meta_path.exists());
        assert!(!reservation_path.exists());
        let content_files = walk_files(&base.join("content-v2"));
        assert!(
            content_files.is_empty(),
            "orphaned blobs: {content_files:?}"
        );
    }

    fn walk_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let Ok(entries) = fs::read_dir(root) else {
            return files;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk_files(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    fn write_partial_record(
        dir: &Path,
        bytes: &[u8],
        url: &str,
        etag: Option<&str>,
        total_size: Option<u64>,
    ) -> (PathBuf, PathBuf) {
        let data_path = dir.join("download.partial");
        let meta_path = dir.join("download.meta");
        fs::write(&data_path, bytes).unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: etag.map(str::to_string),
                last_modified: None,
                total_size,
            },
        )
        .unwrap();
        (data_path, meta_path)
    }

    #[test]
    fn legacy_meta_defaults_missing_total_to_unbound() {
        let meta: PartialMeta = serde_json::from_str(
            r#"{"url":"https://example.test/file","etag":"\"v1\"","last_modified":null}"#,
        )
        .unwrap();

        assert_eq!(meta.total_size, None);
    }

    #[test]
    fn valid_resume_requires_exact_url_strong_etag_and_interior_offset() {
        let dir = tempfile::tempdir().unwrap();
        let (data_path, meta_path) = write_partial_record(
            dir.path(),
            b"abc",
            "https://example.test/file",
            Some("\"v1\""),
            Some(6),
        );

        let resume = prepare_resume_record(&data_path, &meta_path, "https://example.test/file")
            .unwrap()
            .unwrap();

        assert_eq!(resume.offset, 3);
        assert_eq!(resume.total_size, 6);
        assert_eq!(resume.etag, "\"v1\"");
        assert!(data_path.exists());
        assert!(meta_path.exists());
    }

    #[test]
    fn invalid_resume_records_discard_both_sidecars() {
        let cases = [
            ("wrong URL", Some("\"v1\""), Some(6), 3_u64),
            ("weak ETag", Some("W/\"v1\""), Some(6), 3),
            ("missing ETag", None, Some(6), 3),
            ("unquoted ETag", Some("v1"), Some(6), 3),
            ("malformed quoted ETag", Some("\"v1\"tail\""), Some(6), 3),
            ("missing total", Some("\"v1\""), None, 3),
            ("offset equals total", Some("\"v1\""), Some(3), 3),
            ("offset exceeds total", Some("\"v1\""), Some(2), 3),
            ("zero offset", Some("\"v1\""), Some(6), 0),
        ];

        for (label, etag, total_size, offset) in cases {
            let dir = tempfile::tempdir().unwrap();
            let requested_url = "https://example.test/file";
            let stored_url = if label == "wrong URL" {
                "https://other.test/file"
            } else {
                requested_url
            };
            let bytes = vec![b'x'; offset as usize];
            let (data_path, meta_path) =
                write_partial_record(dir.path(), &bytes, stored_url, etag, total_size);

            assert!(
                prepare_resume_record(&data_path, &meta_path, requested_url)
                    .unwrap()
                    .is_none(),
                "{label} must restart cleanly"
            );
            assert!(!data_path.exists(), "{label} left partial bytes behind");
            assert!(!meta_path.exists(), "{label} left metadata behind");
        }
    }

    #[test]
    fn unbound_or_corrupt_partial_is_discarded_instead_of_resumed() {
        for meta_contents in [None, Some(b"not json".as_slice())] {
            let dir = tempfile::tempdir().unwrap();
            let data_path = dir.path().join("download.partial");
            let meta_path = dir.path().join("download.meta");
            fs::write(&data_path, b"unbound bytes").unwrap();
            if let Some(contents) = meta_contents {
                fs::write(&meta_path, contents).unwrap();
            }

            assert!(
                prepare_resume_record(&data_path, &meta_path, "https://example.test/file")
                    .unwrap()
                    .is_none()
            );
            assert!(!data_path.exists());
            assert!(!meta_path.exists());
        }
    }

    #[test]
    fn orphan_metadata_is_discarded_as_an_incomplete_record() {
        let dir = tempfile::tempdir().unwrap();
        let data_path = dir.path().join("download.partial");
        let meta_path = dir.path().join("download.meta");
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some("https://example.test/file".to_string()),
                url: "https://example.test/file".to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        assert!(
            prepare_resume_record(&data_path, &meta_path, "https://example.test/file")
                .unwrap()
                .is_none()
        );
        assert!(!data_path.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn identity_registry_serializes_same_key_but_not_different_keys() {
        let dir = tempfile::tempdir().unwrap();
        let first = download_identity_lock(dir.path(), "same-key").unwrap();
        let same = download_identity_lock(&dir.path().join("."), "same-key").unwrap();
        let different = download_identity_lock(dir.path(), "different-key").unwrap();

        assert!(std::sync::Arc::ptr_eq(&first.state, &same.state));
        assert!(!std::sync::Arc::ptr_eq(&first.state, &different.state));

        let held = first.state.gate.lock();
        assert!(same.state.gate.try_lock().is_none());
        assert!(different.state.gate.try_lock().is_some());
        drop(held);
        assert!(same.state.gate.try_lock().is_some());
    }

    #[test]
    fn identity_registry_removes_weak_entry_after_last_participant() {
        let dir = tempfile::tempdir().unwrap();
        let lock = download_identity_lock(dir.path(), "one-shot-key").unwrap();
        let identity = lock.identity.clone();

        assert!(download_lock_registry().lock().contains_key(&identity));
        drop(lock);
        assert!(!download_lock_registry().lock().contains_key(&identity));
    }

    #[test]
    fn identity_registry_removes_entry_after_concurrent_final_drops() {
        let dir = tempfile::tempdir().unwrap();
        let first = download_identity_lock(dir.path(), "concurrent-final-drops").unwrap();
        let second = download_identity_lock(dir.path(), "concurrent-final-drops").unwrap();
        let identity = first.identity.clone();

        assert_eq!(first.state.participants.load(Ordering::SeqCst), 2);

        let ready = Arc::new(std::sync::Barrier::new(3));
        let first_ready = ready.clone();
        let first_drop = std::thread::spawn(move || {
            first_ready.wait();
            drop(first);
        });
        let second_ready = ready.clone();
        let second_drop = std::thread::spawn(move || {
            second_ready.wait();
            drop(second);
        });

        ready.wait();
        first_drop.join().unwrap();
        second_drop.join().unwrap();

        assert!(!download_lock_registry().lock().contains_key(&identity));
    }

    #[test]
    fn identity_registry_keeps_entry_after_non_final_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = download_identity_lock(dir.path(), "non-final-drop").unwrap();
        let second = download_identity_lock(dir.path(), "non-final-drop").unwrap();
        let identity = first.identity.clone();

        drop(first);

        let retained = download_lock_registry()
            .lock()
            .get(&identity)
            .and_then(Weak::upgrade)
            .expect("non-final drop removed a live download state");
        assert!(Arc::ptr_eq(&retained, &second.state));

        drop(retained);
        drop(second);
        assert!(!download_lock_registry().lock().contains_key(&identity));
    }

    #[test]
    fn valid_strong_etag_record_resumes_and_commits_combined_bytes() {
        let (_dir, cache, index) = test_cache();
        let key = "resume-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let mut calls = 0;
        let bytes = fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match| {
                assert!(
                    !meta_path.exists(),
                    "resume metadata must be revoked before appending new bytes"
                );
                calls += 1;
                assert_eq!(range_start, Some(3));
                assert_eq!(if_match, Some("\"v1\""));
                writer.write_all(b"def").unwrap();
                Ok(streaming_result(3, true, Some("\"v1\""), Some(6)))
            },
        )
        .unwrap();

        assert_eq!(calls, 1);
        assert_eq!(bytes, 6);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"abcdef");
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
        assert!(!data_path.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn interrupted_fresh_stream_binds_sidecar_before_bytes_and_resumes_exact_suffix() {
        let (_dir, cache, index) = test_cache();
        let key = "bound-interrupted-key";
        let requested_url = "https://origin.example/file";
        let validated_url = "https://cdn.example/file";
        let (data_path, meta_path) = partial_paths(&cache, key);

        let first = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            requested_url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match, bind_metadata| {
                assert_eq!(range_start, None);
                assert_eq!(if_match, None);
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: validated_url.to_string(),
                    was_partial: false,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: Some(6),
                    expected_body_length: Some(6),
                })?;
                assert!(meta_path.exists(), "body started before sidecar binding");
                writer.write_all(b"abc").unwrap();
                Err("connection closed after three bytes".to_string())
            },
        );

        assert!(first.is_err());
        assert_eq!(fs::read(&data_path).unwrap(), b"abc");
        let bound = read_meta(&meta_path).expect("interrupted stream retained bound metadata");
        assert_eq!(bound.requested_url.as_deref(), Some(requested_url));
        assert_eq!(bound.url, validated_url);
        assert_eq!(bound.etag.as_deref(), Some("\"v1\""));
        assert_eq!(bound.total_size, Some(6));

        let mut calls = 0;
        let bytes = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            requested_url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match, bind_metadata| {
                calls += 1;
                assert_eq!(range_start, Some(3));
                assert_eq!(if_match, Some("\"v1\""));
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: validated_url.to_string(),
                    was_partial: true,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: Some(3),
                    total_size: Some(6),
                    expected_body_length: Some(3),
                })?;
                writer.write_all(b"def").unwrap();
                Ok(streaming_result(3, true, Some("\"v1\""), Some(6)))
            },
        )
        .unwrap();

        assert_eq!(calls, 1, "resume redownloaded the prefix");
        assert_eq!(bytes, 6);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"abcdef");
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
        assert!(!data_path.exists());
        assert!(!meta_path.exists());
    }

    #[test]
    fn resumable_streaming_accepts_a_body_above_the_materialization_limit() {
        const TOTAL: u64 = 50 * 1024 * 1024 + 1;
        const PREFIX: u64 = 1024 * 1024;
        let (_dir, cache, index) = test_cache();
        let key = "large-resumable-video";
        let url = "https://example.test/large-video";

        let first = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match, bind_metadata| {
                assert_eq!(range_start, None);
                assert_eq!(if_match, None);
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: url.to_string(),
                    was_partial: false,
                    etag: Some("\"large-v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: Some(TOTAL),
                    expected_body_length: Some(TOTAL),
                })?;
                let chunk = [0xA5; 64 * 1024];
                for _ in 0..PREFIX / chunk.len() as u64 {
                    writer.write_all(&chunk).unwrap();
                }
                Err("injected interruption".to_string())
            },
        );
        assert!(first.is_err());

        let bytes = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match, bind_metadata| {
                assert_eq!(range_start, Some(PREFIX));
                assert_eq!(if_match, Some("\"large-v1\""));
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: url.to_string(),
                    was_partial: true,
                    etag: Some("\"large-v1\"".to_string()),
                    last_modified: None,
                    range_start: Some(PREFIX),
                    total_size: Some(TOTAL),
                    expected_body_length: Some(TOTAL - PREFIX),
                })?;
                let chunk = [0xA5; 64 * 1024];
                let mut remaining = TOTAL - PREFIX;
                while remaining > 0 {
                    let length = usize::try_from(remaining.min(chunk.len() as u64)).unwrap();
                    writer.write_all(&chunk[..length]).unwrap();
                    remaining -= length as u64;
                }
                Ok(streaming_result(
                    TOTAL - PREFIX,
                    true,
                    Some("\"large-v1\""),
                    Some(TOTAL),
                ))
            },
        )
        .unwrap();

        assert_eq!(bytes, TOTAL);
        assert!(
            cache.get(key).is_err(),
            "large body must remain streaming-only"
        );
        let entry = index.entry.lock().clone().unwrap();
        assert_eq!(entry.size_bytes, Some(TOTAL as i64));
        let sri: ssri::Integrity = entry.content_hash.parse().unwrap();
        let mut reader = cacache::SyncReader::open_hash(cache.base_dir(), sri).unwrap();
        let mut observed = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            assert!(buffer[..read].iter().all(|byte| *byte == 0xA5));
            observed += read as u64;
        }
        reader.check().unwrap();
        assert_eq!(observed, TOTAL);
    }

    #[test]
    fn checked_plugin_interrupted_stream_resumes_end_to_end() {
        let (_dir, cache, index) = test_cache();
        let key = "checked-loopback-resume";
        let plugin_id = "checked-resume-plugin";
        let requested_url = "http://1.1.1.1/resume";
        let owner = CacheOwner::plugin(plugin_id);
        let (data_path, meta_path) = partial_paths_for_owner(&cache, &owner, key);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local SOCKS5 server");
        let proxy_address = listener.local_addr().expect("local SOCKS5 address");
        let server_meta_path = meta_path.clone();
        let server_url = requested_url.to_string();

        let server = std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept SOCKS5 connection");
                let request = read_socks_http_request(&mut stream);
                let normalized = request.to_ascii_lowercase();

                if attempt == 0 {
                    assert!(
                        !normalized.contains("\r\nrange:"),
                        "fresh checked request unexpectedly sent Range: {request:?}"
                    );
                    assert!(
                        !normalized.contains("\r\nif-match:"),
                        "fresh checked request unexpectedly sent If-Match: {request:?}"
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
                        )
                        .expect("write interrupted response headers");
                    stream.flush().expect("flush interrupted response headers");

                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !server_meta_path.exists() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let metadata: serde_json::Value = serde_json::from_slice(
                        &fs::read(&server_meta_path)
                            .expect("sidecar was not bound before the first body byte"),
                    )
                    .expect("bound sidecar was invalid JSON");
                    assert_eq!(metadata["requested_url"], server_url);
                    assert_eq!(metadata["url"], server_url);
                    assert_eq!(metadata["etag"], "\"v1\"");
                    assert_eq!(metadata["total_size"], 6);

                    stream
                        .write_all(b"abc")
                        .expect("write interrupted response prefix");
                } else {
                    assert!(
                        normalized.contains("\r\nrange: bytes=3-\r\n"),
                        "resume did not send the exact Range header: {request:?}"
                    );
                    assert!(
                        normalized.contains("\r\nif-match: \"v1\"\r\n"),
                        "resume did not send the exact If-Match header: {request:?}"
                    );
                    stream
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"v1\"\r\nConnection: close\r\n\r\ndef",
                        )
                        .expect("write resumed response");
                }
                stream.flush().expect("flush SOCKS5 HTTP response");
            }
        });

        let runtime = tokio::runtime::Runtime::new().expect("build checked-fetch runtime");
        let client = checked_plugin_client(&runtime, Some(proxy_address), plugin_id, true);

        let first = fetch_url_to_cache_for_plugin(
            &cache,
            &client,
            key,
            requested_url,
            CacheType::Other,
            None,
            plugin_id,
        );
        assert!(
            first.is_err(),
            "interrupted checked fetch unexpectedly succeeded"
        );
        assert_eq!(fs::read(&data_path).unwrap(), b"abc");
        assert!(
            meta_path.exists(),
            "interrupted checked fetch lost its sidecar"
        );

        let bytes = fetch_url_to_cache_for_plugin(
            &cache,
            &client,
            key,
            requested_url,
            CacheType::Other,
            None,
            plugin_id,
        )
        .expect("checked fetch did not resume its validated suffix");

        assert_eq!(bytes, 6);
        assert_eq!(
            cache.get_for_owner(&owner, key).unwrap().unwrap(),
            b"abcdef"
        );
        assert!(cache.get(key).unwrap().is_none());
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
        assert!(!data_path.exists());
        assert!(!meta_path.exists());
        server.join().expect("local SOCKS5 server panicked");
    }

    #[test]
    fn checked_plugin_policy_rejection_never_falls_back_to_host_streaming() {
        let (_dir, cache, index) = test_cache();
        let key = "checked-policy-rejection";
        let plugin_id = "disabled-network-plugin";
        let owner = CacheOwner::plugin(plugin_id);
        let (partial_path, meta_path) = partial_paths_for_owner(&cache, &owner, key);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind host-fallback sentinel");
        listener
            .set_nonblocking(true)
            .expect("make host-fallback sentinel nonblocking");
        let address = listener
            .local_addr()
            .expect("host-fallback sentinel address");
        let url = format!("http://{address}/must-not-connect");
        let runtime = tokio::runtime::Runtime::new().expect("build checked-fetch runtime");
        let client = checked_plugin_client(&runtime, None, plugin_id, false);

        let error = fetch_url_to_cache_for_plugin(
            &cache,
            &client,
            key,
            &url,
            CacheType::Other,
            None,
            plugin_id,
        )
        .expect_err("disabled plugin network policy unexpectedly fetched data");

        assert!(
            error
                .to_ascii_lowercase()
                .contains("network capability is disabled"),
            "{error}"
        );
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(
            partial_path.exists(),
            "checked fetch did not open its body sink"
        );
        assert_eq!(fs::metadata(&partial_path).unwrap().len(), 0);
        assert!(
            !meta_path.exists(),
            "policy rejection persisted response metadata"
        );
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
        assert!(cache.get(key).unwrap().is_none());
    }

    #[test]
    fn interrupted_weak_or_missing_etag_restarts_without_a_range() {
        for (label, etag) in [("weak", Some("W/\"v1\"")), ("missing", None)] {
            let (_dir, cache, _index) = test_cache();
            let key = format!("{label}-etag-key");
            let url = "https://example.test/file";

            let first = fetch_url_to_cache_with_metadata(
                &cache,
                &key,
                url,
                CacheType::Other,
                None,
                false,
                |writer, range_start, if_match, bind_metadata| {
                    assert_eq!(range_start, None);
                    assert_eq!(if_match, None);
                    bind_metadata(&StreamingResponseMetadata {
                        validated_url: url.to_string(),
                        was_partial: false,
                        etag: etag.map(str::to_string),
                        last_modified: None,
                        range_start: None,
                        total_size: Some(6),
                        expected_body_length: Some(6),
                    })?;
                    writer.write_all(b"abc").unwrap();
                    Err("interrupted".to_string())
                },
            );
            assert!(first.is_err());

            let bytes = fetch_url_to_cache_with_metadata(
                &cache,
                &key,
                url,
                CacheType::Other,
                None,
                false,
                |writer, range_start, if_match, bind_metadata| {
                    assert_eq!(range_start, None, "{label} ETag authorized a range");
                    assert_eq!(if_match, None, "{label} ETag authorized If-Match");
                    bind_metadata(&StreamingResponseMetadata {
                        validated_url: url.to_string(),
                        was_partial: false,
                        etag: Some("\"v2\"".to_string()),
                        last_modified: None,
                        range_start: None,
                        total_size: Some(6),
                        expected_body_length: Some(6),
                    })?;
                    writer.write_all(b"fresh!").unwrap();
                    Ok(streaming_result(6, false, Some("\"v2\""), Some(6)))
                },
            )
            .unwrap();

            assert_eq!(bytes, 6);
            assert_eq!(cache.get(&key).unwrap().unwrap(), b"fresh!");
        }
    }

    #[test]
    fn sidecar_install_failure_aborts_before_body_bytes() {
        let (_dir, cache, index) = test_cache();
        let key = "sidecar-failure-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);

        let result = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, _range_start, _if_match, bind_metadata| {
                fs::create_dir(&meta_path).expect("inject metadata destination failure");
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: url.to_string(),
                    was_partial: false,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: None,
                    total_size: Some(6),
                    expected_body_length: Some(6),
                })?;
                writer.write_all(b"unsafe!").unwrap();
                Ok(streaming_result(7, false, Some("\"v1\""), Some(7)))
            },
        );

        assert!(result
            .unwrap_err()
            .contains("binding partial response metadata"));
        assert_eq!(fs::metadata(&data_path).unwrap().len(), 0);
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn changed_redirect_identity_rejects_resume_before_appending() {
        let (_dir, cache, index) = test_cache();
        let key = "changed-redirect-key";
        let requested_url = "https://origin.example/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(requested_url.to_string()),
                url: "https://cdn-one.example/file".to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let result = fetch_url_to_cache_with_metadata(
            &cache,
            key,
            requested_url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match, bind_metadata| {
                assert_eq!(range_start, Some(3));
                assert_eq!(if_match, Some("\"v1\""));
                bind_metadata(&StreamingResponseMetadata {
                    validated_url: "https://cdn-two.example/file".to_string(),
                    was_partial: true,
                    etag: Some("\"v1\"".to_string()),
                    last_modified: None,
                    range_start: Some(3),
                    total_size: Some(6),
                    expected_body_length: Some(3),
                })?;
                writer.write_all(b"def").unwrap();
                Ok(streaming_result(3, true, Some("\"v1\""), Some(6)))
            },
        );

        assert!(result
            .unwrap_err()
            .contains("does not match the bound representation"));
        assert_eq!(fs::read(&data_path).unwrap(), b"abc");
        assert!(
            !meta_path.exists(),
            "old redirect binding remained authoritative"
        );
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn resumed_total_mismatch_truncates_and_restarts_once_without_range() {
        let (_dir, cache, index) = test_cache();
        let key = "restart-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let mut calls = 0;
        let bytes = fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match| {
                calls += 1;
                match calls {
                    1 => {
                        assert_eq!(range_start, Some(3));
                        assert_eq!(if_match, Some("\"v1\""));
                        writer.write_all(b"xyz").unwrap();
                        Ok(streaming_result(3, true, Some("\"v1\""), Some(7)))
                    }
                    2 => {
                        assert_eq!(range_start, None);
                        assert_eq!(if_match, None);
                        writer.write_all(b"fresh!").unwrap();
                        Ok(streaming_result(6, false, Some("\"v2\""), Some(6)))
                    }
                    _ => panic!("resume mismatch restarted more than once"),
                }
            },
        )
        .unwrap();

        assert_eq!(calls, 2);
        assert_eq!(bytes, 6);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"fresh!");
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn resumed_response_without_matching_strong_etag_restarts_cleanly() {
        let (_dir, cache, _index) = test_cache();
        let key = "etag-restart-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let mut calls = 0;
        fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, _if_match| {
                calls += 1;
                if calls == 1 {
                    assert_eq!(range_start, Some(3));
                    writer.write_all(b"old").unwrap();
                    Ok(streaming_result(3, true, None, Some(6)))
                } else {
                    assert_eq!(range_start, None);
                    writer.write_all(b"fresh!").unwrap();
                    Ok(streaming_result(6, false, Some("\"v2\""), Some(6)))
                }
            },
        )
        .unwrap();

        assert_eq!(calls, 2);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"fresh!");
    }

    #[test]
    fn resumed_stream_error_discards_unverified_appends_before_clean_restart() {
        let (_dir, cache, _index) = test_cache();
        let key = "error-restart-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let mut calls = 0;
        let bytes = fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match| {
                calls += 1;
                if calls == 1 {
                    assert_eq!(range_start, Some(3));
                    assert_eq!(if_match, Some("\"v1\""));
                    writer.write_all(b"foreign").unwrap();
                    Err("stream failed after an append".to_string())
                } else {
                    assert_eq!(range_start, None);
                    assert_eq!(if_match, None);
                    writer.write_all(b"fresh!").unwrap();
                    Ok(streaming_result(6, false, Some("\"v2\""), Some(6)))
                }
            },
        )
        .unwrap();

        assert_eq!(calls, 2);
        assert_eq!(bytes, 6);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"fresh!");
    }

    #[test]
    fn failed_data_cleanup_after_invalid_resume_cannot_reauthorize_appended_bytes() {
        let (_dir, cache, index) = test_cache();
        let key = "failed-cleanup-key";
        let url = "https://example.test/file";
        let (data_path, meta_path) = partial_paths(&cache, key);
        fs::create_dir_all(data_path.parent().unwrap()).unwrap();
        fs::write(&data_path, b"abc").unwrap();
        write_meta(
            &meta_path,
            &PartialMeta {
                requested_url: Some(url.to_string()),
                url: url.to_string(),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
                total_size: Some(6),
            },
        )
        .unwrap();

        let mut data_removal_attempts = 0;
        let first = fetch_url_to_cache_with_file_ops(
            &cache,
            key,
            DownloadRequestDescriptor {
                url: url.to_string(),
                cache_type: CacheType::Other,
                product_id: None,
                use_proxy: false,
                plugin_id: None,
            },
            |writer, range_start, if_match| {
                assert_eq!(range_start, Some(3));
                assert_eq!(if_match, Some("\"v1\""));
                assert!(
                    !meta_path.exists(),
                    "metadata must be revoked before the transport can append"
                );
                writer.write_all(b"x").unwrap();
                Ok(streaming_result(1, true, Some("\"v1\""), Some(7)))
            },
            |path| {
                if path == data_path && data_removal_attempts == 0 {
                    data_removal_attempts += 1;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected partial-data removal failure",
                    ));
                }
                fs::remove_file(path)
            },
        );

        assert!(first
            .unwrap_err()
            .contains("injected partial-data removal failure"));
        assert_eq!(data_removal_attempts, 1);
        assert!(
            data_path.exists(),
            "injected failure should leave data behind"
        );
        assert!(
            !meta_path.exists(),
            "failed data cleanup must not leave authoritative metadata"
        );
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);

        let mut next_calls = 0;
        let bytes = fetch_url_to_cache_with(
            &cache,
            key,
            url,
            CacheType::Other,
            None,
            false,
            |writer, range_start, if_match| {
                next_calls += 1;
                assert_eq!(range_start, None);
                assert_eq!(if_match, None);
                writer.write_all(b"fresh!").unwrap();
                Ok(streaming_result(6, false, Some("\"v2\""), Some(6)))
            },
        )
        .unwrap();

        assert_eq!(next_calls, 1);
        assert_eq!(bytes, 6);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"fresh!");
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_same_identity_coalesces_through_index_commit_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("cache");
        fs::create_dir_all(&base).unwrap();
        let index = Arc::new(RecordingIndex::default());
        let (upsert_started_tx, upsert_started_rx) = std::sync::mpsc::channel();
        let (upsert_release_tx, upsert_release_rx) = std::sync::mpsc::channel();
        *index.first_upsert_started.lock() = Some(upsert_started_tx);
        *index.first_upsert_release.lock() = Some(upsert_release_rx);
        let cache = ContentCache::new_with_limits(
            base.clone(),
            index.clone(),
            CacheLimits {
                min_free_space_bytes: 0,
                ..CacheLimits::default()
            },
        )
        .unwrap();
        let key = "concurrent-key";
        let url = "https://example.test/file";
        let fetch_calls = Arc::new(AtomicUsize::new(0));

        let first_cache = cache.clone();
        let first_calls = fetch_calls.clone();
        let first = std::thread::spawn(move || {
            fetch_url_to_cache_with(
                &first_cache,
                key,
                url,
                CacheType::Other,
                None,
                false,
                |writer, range_start, if_match| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(range_start, None);
                    assert_eq!(if_match, None);
                    writer.write_all(b"one").unwrap();
                    Ok(streaming_result(3, false, Some("\"v1\""), Some(3)))
                },
            )
        });

        upsert_started_rx.recv().unwrap();

        let second_cache = cache.clone();
        let second_calls = fetch_calls.clone();
        let second = std::thread::spawn(move || {
            fetch_url_to_cache_with(
                &second_cache,
                key,
                url,
                CacheType::Other,
                None,
                false,
                |writer, _range_start, _if_match| {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    writer.write_all(b"two").unwrap();
                    Ok(streaming_result(3, false, Some("\"v2\""), Some(3)))
                },
            )
        });

        let identity = DownloadIdentity {
            cache_base_dir: fs::canonicalize(&base).unwrap(),
            cache_key: CacheOwner::host().scoped_key(key),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let participant_count = download_lock_registry()
                .lock()
                .get(&identity)
                .map(Weak::strong_count)
                .unwrap_or(0);
            if participant_count >= 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "second caller did not join the identity lock"
            );
            std::thread::yield_now();
        }

        assert_eq!(fetch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(index.upserts.load(Ordering::SeqCst), 0);
        upsert_release_tx.send(()).unwrap();

        assert_eq!(first.join().unwrap().unwrap(), 3);
        assert_eq!(second.join().unwrap().unwrap(), 3);
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 1);
        assert_eq!(index.upserts.load(Ordering::SeqCst), 1);
        assert_eq!(cache.get(key).unwrap().unwrap(), b"one");
    }

    #[test]
    fn concurrent_same_key_only_coalesces_exact_request_descriptors() {
        let cases = [
            (
                "URL",
                "https://example.test/one",
                CacheType::Other,
                None,
                false,
                "https://example.test/two",
                CacheType::Other,
                None,
                false,
            ),
            (
                "cache type",
                "https://example.test/file",
                CacheType::Other,
                None,
                false,
                "https://example.test/file",
                CacheType::Cover,
                None,
                false,
            ),
            (
                "product ID",
                "https://example.test/file",
                CacheType::Other,
                Some("product-one"),
                false,
                "https://example.test/file",
                CacheType::Other,
                Some("product-two"),
                false,
            ),
            (
                "transport mode",
                "https://example.test/file",
                CacheType::Other,
                None,
                false,
                "https://example.test/file",
                CacheType::Other,
                None,
                true,
            ),
        ];

        for (
            label,
            first_url,
            first_cache_type,
            first_product_id,
            first_use_proxy,
            second_url,
            second_cache_type,
            second_product_id,
            second_use_proxy,
        ) in cases
        {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().join("cache");
            fs::create_dir_all(&base).unwrap();
            let index = Arc::new(RecordingIndex::default());
            let (upsert_started_tx, upsert_started_rx) = std::sync::mpsc::channel();
            let (upsert_release_tx, upsert_release_rx) = std::sync::mpsc::channel();
            *index.first_upsert_started.lock() = Some(upsert_started_tx);
            *index.first_upsert_release.lock() = Some(upsert_release_rx);
            let cache = ContentCache::new_with_limits(
                base.clone(),
                index.clone(),
                CacheLimits {
                    min_free_space_bytes: 0,
                    ..CacheLimits::default()
                },
            )
            .unwrap();
            let key = "descriptor-key";
            let fetch_calls = Arc::new(AtomicUsize::new(0));

            let first_cache = cache.clone();
            let first_calls = fetch_calls.clone();
            let first = std::thread::spawn(move || {
                fetch_url_to_cache_with(
                    &first_cache,
                    key,
                    first_url,
                    first_cache_type,
                    first_product_id,
                    first_use_proxy,
                    |writer, range_start, if_match| {
                        first_calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(range_start, None);
                        assert_eq!(if_match, None);
                        writer.write_all(b"one").unwrap();
                        Ok(streaming_result(3, false, Some("\"v1\""), Some(3)))
                    },
                )
            });

            upsert_started_rx.recv().unwrap();

            let second_cache = cache.clone();
            let second_calls = fetch_calls.clone();
            let second = std::thread::spawn(move || {
                fetch_url_to_cache_with(
                    &second_cache,
                    key,
                    second_url,
                    second_cache_type,
                    second_product_id,
                    second_use_proxy,
                    |writer, range_start, if_match| {
                        second_calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(range_start, None);
                        assert_eq!(if_match, None);
                        writer.write_all(b"two").unwrap();
                        Ok(streaming_result(3, false, Some("\"v2\""), Some(3)))
                    },
                )
            });

            let identity = DownloadIdentity {
                cache_base_dir: fs::canonicalize(&base).unwrap(),
                cache_key: CacheOwner::host().scoped_key(key),
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let participant_count = download_lock_registry()
                    .lock()
                    .get(&identity)
                    .map(Weak::strong_count)
                    .unwrap_or(0);
                if participant_count >= 2 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "second {label} caller did not join the identity lock"
                );
                std::thread::yield_now();
            }

            upsert_release_tx.send(()).unwrap();
            assert_eq!(first.join().unwrap().unwrap(), 3, "{label}");
            assert_eq!(second.join().unwrap().unwrap(), 3, "{label}");
            assert_eq!(fetch_calls.load(Ordering::SeqCst), 2, "{label}");
            assert_eq!(index.upserts.load(Ordering::SeqCst), 2, "{label}");
            assert_eq!(cache.get(key).unwrap().unwrap(), b"two", "{label}");

            let entry = index.entry.lock().clone().unwrap();
            assert_eq!(entry.source_url.as_deref(), Some(second_url), "{label}");
            assert_eq!(entry.cache_type, second_cache_type, "{label}");
            assert_eq!(entry.product_id.as_deref(), second_product_id, "{label}");
        }
    }
}
