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
//! Resume: if `.partial` exists when [`fetch_url_to_cache`] is called
//! again for the same key, we send `Range: bytes=N-` (with the stored
//! ETag as `If-Match` for validation). A 206 response appends to the
//! existing partial; a 200 response (range rejected) means we truncate
//! and start over from byte 0 — same end state, just with the wasted
//! bytes from the prior attempt thrown away.

use crate::features::content_cache::ContentCache;
use anyhow::{Context, Result};
use arclain_db::CacheType;
use arclain_network::AsyncHttpClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Sidecar metadata stored next to `<keyhash>.partial`, used to
/// validate resumes (ETag / Last-Modified) and to remember the source
/// URL across attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartialMeta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
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
        fs::create_dir_all(parent).with_context(|| {
            format!("creating partial dir {:?}", parent)
        })?;
    }
    fs::write(meta_path, json).with_context(|| {
        format!("writing partial meta {:?}", meta_path)
    })
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
    let (partial_path, meta_path) = partial_paths(cache, key);
    fs::create_dir_all(partial_dir(cache).as_path())
        .map_err(|e| format!("creating partial dir: {}", e))?;

    // Existing partial? Read sidecar to get the prior etag for
    // resume validation, then size to get the resume offset.
    let prior_meta = read_meta(&meta_path);
    let start_byte: u64 = fs::metadata(&partial_path)
        .map(|m| m.len())
        .unwrap_or(0);

    if start_byte > 0 {
        info!(
            "[streaming] resuming {} from byte {} (etag: {:?})",
            key,
            start_byte,
            prior_meta.as_ref().and_then(|m| m.etag.as_deref())
        );
    }

    // Open the partial for append. If the server returns 200 (range
    // rejected), we'll detect that, truncate, and re-stream below.
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial_path)
        .map_err(|e| format!("opening partial file: {}", e))?;

    let range_start = if start_byte > 0 { Some(start_byte) } else { None };
    let if_match = prior_meta.as_ref().and_then(|m| m.etag.clone());
    let mut result = http_client.blocking_get_streaming(
        url,
        use_proxy,
        &mut file,
        range_start,
        if_match.as_deref(),
    )?;

    // Server returned 200 (full body) when we asked for a range →
    // the partial now has [old bytes 0..start] + [new bytes 0..n],
    // which is garbage. Truncate and re-stream from scratch.
    if !result.was_partial && start_byte > 0 {
        warn!(
            "[streaming] server ignored range request for {} (status 200); restarting",
            key
        );
        drop(file);
        let mut fresh = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&partial_path)
            .map_err(|e| format!("re-opening partial file: {}", e))?;
        result = http_client.blocking_get_streaming(
            url,
            use_proxy,
            &mut fresh,
            None,
            None,
        )?;
    }

    // Persist updated meta so a future resume sees the right etag.
    let meta = PartialMeta {
        url: url.to_string(),
        etag: result.etag.clone(),
        last_modified: result.last_modified.clone(),
    };
    if let Err(e) = write_meta(&meta_path, &meta) {
        debug!("[streaming] failed to write meta sidecar: {}", e);
    }

    // Collapse the .partial file into cacache. We stream the partial's
    // contents through cacache::SyncWriter so we don't load it into
    // memory; this is the second pass over the bytes (the first being
    // the network → .partial pass).
    let partial_size = fs::metadata(&partial_path)
        .map(|m| m.len())
        .unwrap_or(0);
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
            cache_type,
            product_id,
            Some(url),
        )
        .map_err(|e| format!("upserting cache index: {}", e))?;

    // Cleanup partial sidecars on success. Best-effort — orphans get
    // GC'd by `.partial` directory cleanup on next run if removal
    // fails (e.g. on Windows when antivirus is mid-scan).
    let _ = fs::remove_file(&partial_path);
    let _ = fs::remove_file(&meta_path);

    debug!(
        "[streaming] cached {} bytes for key {} (partial was {}, sri {})",
        bytes_committed, key, partial_size, sri
    );
    Ok(bytes_committed)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        write_meta(&path, &meta).unwrap();
        let read = read_meta(&path).unwrap();
        assert_eq!(read.url, meta.url);
        assert_eq!(read.etag, meta.etag);
        assert_eq!(read.last_modified, meta.last_modified);
    }

    #[test]
    fn read_missing_meta_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.meta");
        assert!(read_meta(&path).is_none());
    }
}
