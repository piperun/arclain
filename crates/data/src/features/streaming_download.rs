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
//! The current network API returns response validators only after a stream
//! completes. Consequently, bytes left by an interrupted first request are
//! intentionally unbound and will be restarted rather than resumed.

use crate::features::content_cache::ContentCache;
use anyhow::{Context, Result};
use arclain_db::CacheType;
use arclain_network::{AsyncHttpClient, StreamingDownload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use tracing::{debug, info, warn};

use parking_lot::Mutex;

/// Sidecar metadata stored next to `<keyhash>.partial`.
///
/// URL, strong ETag, total size, and the current partial length form the
/// resume identity. Last-Modified is retained for diagnostics/compatibility,
/// but is not strong enough to authorize a resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartialMeta {
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
        if Arc::strong_count(&self.state) != 1 {
            return;
        }

        let mut registry = download_lock_registry().lock();
        let current = registry.get(&self.identity);
        let points_to_state =
            current.is_some_and(|weak| Weak::ptr_eq(weak, &Arc::downgrade(&self.state)));
        if points_to_state && Arc::strong_count(&self.state) == 1 {
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
            });
            registry.insert(identity.clone(), Arc::downgrade(&state));
            state
        });

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

fn partial_paths(cache: &ContentCache, key: &str) -> (PathBuf, PathBuf) {
    let dir = partial_dir(cache);
    let name = key_to_sidecar_name(key);
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
    fs::write(meta_path, json).with_context(|| format!("writing partial meta {:?}", meta_path))
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
        if meta.url != requested_url
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
    fetch_url_to_cache_with(
        cache,
        key,
        url,
        cache_type,
        product_id,
        use_proxy,
        |writer, range_start, if_match| {
            http_client.blocking_get_streaming(url, use_proxy, writer, range_start, if_match)
        },
    )
}

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
    F: FnMut(&mut File, Option<u64>, Option<&str>) -> Result<StreamingDownload, String>,
{
    let request = DownloadRequestDescriptor {
        url: url.to_string(),
        cache_type,
        product_id: product_id.map(str::to_string),
        use_proxy,
    };
    fetch_url_to_cache_with_file_ops(cache, key, request, fetch, |path| fs::remove_file(path))
}

fn fetch_url_to_cache_with_file_ops<F, R>(
    cache: &ContentCache,
    key: &str,
    request_descriptor: DownloadRequestDescriptor,
    mut fetch: F,
    mut remove_file: R,
) -> Result<u64, String>
where
    F: FnMut(&mut File, Option<u64>, Option<&str>) -> Result<StreamingDownload, String>,
    R: FnMut(&Path) -> std::io::Result<()>,
{
    let (partial_path, meta_path) = partial_paths(cache, key);
    fs::create_dir_all(partial_dir(cache).as_path())
        .map_err(|e| format!("creating partial dir: {}", e))?;

    let identity_lock = download_identity_lock(cache.base_dir(), key)
        .map_err(|error| format!("locking streaming download identity: {error:#}"))?;
    let _identity_guard = identity_lock.state.gate.lock();
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

    if let Some(record) = resume.as_ref() {
        remove_record_file_with(&meta_path, &mut remove_file)
            .map_err(|error| format!("revoking resume metadata before append: {error:#}"))?;
        info!(
            "[streaming] resuming {} from byte {} (etag: {})",
            key, record.offset, record.etag
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

    let initial_result = fetch(
        &mut file,
        resume.as_ref().map(|record| record.offset),
        resume.as_ref().map(|record| record.etag.as_str()),
    );
    let mut did_restart = false;
    let mut result = match initial_result {
        Ok(result) => result,
        Err(error) if resume.is_some() => {
            warn!(
                "[streaming] resume stream failed for {}; discarding appended bytes before restart",
                key
            );
            drop(file);
            discard_partial_record_with(&partial_path, &meta_path, &mut remove_file).map_err(
                |discard_error| {
                    format!(
                        "discarding partial download after resume error ({error}): {discard_error:#}"
                    )
                },
            )?;
            file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&partial_path)
                .map_err(|open_error| format!("opening clean restart file: {open_error}"))?;
            did_restart = true;
            fetch(&mut file, None, None).map_err(|restart_error| {
                format!("resume failed ({error}); clean restart failed: {restart_error}")
            })?
        }
        Err(error) => return Err(error),
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
            key
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
        result = fetch(&mut file, None, None)?;
        did_restart = true;
    }
    if (resume.is_none() || did_restart) && result.was_partial {
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
        return Err(format!(
            "completed partial size {partial_size} does not match resource total {total_size}"
        ));
    }

    let meta = PartialMeta {
        url: request_descriptor.url.clone(),
        etag: result.etag.clone(),
        last_modified: result.last_modified.clone(),
        total_size: Some(total_size),
    };
    if let Err(e) = write_meta(&meta_path, &meta) {
        debug!("[streaming] failed to write meta sidecar: {}", e);
    }

    drop(file);

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
    let (sri, bytes_committed) = writer
        .commit()
        .map_err(|e| format!("committing cache write: {}", e))?;

    cache
        .upsert_sri(
            key,
            &sri,
            bytes_committed,
            request_descriptor.cache_type,
            request_descriptor.product_id.as_deref(),
            Some(&request_descriptor.url),
        )
        .map_err(|e| format!("upserting cache index: {}", e))?;

    // Cleanup partial sidecars on success. Best-effort — orphans get
    // GC'd by `.partial` directory cleanup on next run if removal
    // fails (e.g. on Windows when antivirus is mid-scan).
    let _ = discard_partial_record_with(&partial_path, &meta_path, &mut remove_file);

    debug!(
        "[streaming] cached {} bytes for key {} (partial was {}, sri {})",
        bytes_committed, key, partial_size, sri
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
    use crate::traits::CacheIndex;
    use arclain_db::CacheEntry;
    use arclain_network::StreamingDownload;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingIndex {
        entry: Mutex<Option<CacheEntry>>,
        upserts: AtomicUsize,
        first_upsert_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        first_upsert_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
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
        let cache = ContentCache::new(base, index.clone()).unwrap();
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
        let cache = ContentCache::new(base.clone(), index.clone()).unwrap();
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
            cache_key: key.to_string(),
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
            let cache = ContentCache::new(base.clone(), index.clone()).unwrap();
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
                cache_key: key.to_string(),
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
