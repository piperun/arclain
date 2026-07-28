use anyhow::Result;
use arclain_data::{
    CacheIndex, ContentCache, ContentCacheResolver, DataRequest, DataService, DataSource,
    DataSourceResolver, DataStatus, ResourceConfig, ResourceManager, ResourceRequest,
    StorageStrategy,
};
use arclain_db::{CacheEntry, CacheType};
use parking_lot::Mutex;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct TestCacheIndex {
    entry: Mutex<Option<CacheEntry>>,
    access_updates: AtomicUsize,
}

impl TestCacheIndex {
    fn set_entry(&self, entry: CacheEntry) {
        *self.entry.lock() = Some(entry);
    }

    fn set_size(&self, size_bytes: Option<i64>) {
        self.entry
            .lock()
            .as_mut()
            .expect("cache entry should exist")
            .size_bytes = size_bytes;
    }
}

impl CacheIndex for TestCacheIndex {
    fn upsert(
        &self,
        key: &str,
        product_id: Option<&str>,
        content_hash: &str,
        source_url: Option<&str>,
        cache_type: CacheType,
        size_bytes: Option<i64>,
    ) -> Result<i64> {
        self.set_entry(CacheEntry {
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
        self.access_updates.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn test_cache() -> (tempfile::TempDir, ContentCache, Arc<TestCacheIndex>) {
    let directory = tempfile::tempdir().expect("create cache tempdir");
    let cache_dir = directory.path().join("content");
    fs::create_dir_all(&cache_dir).expect("create cache directory");
    let index = Arc::new(TestCacheIndex::default());
    let cache = ContentCache::new_with_limits(
        cache_dir,
        index.clone(),
        arclain_data::CacheLimits {
            min_free_space_bytes: 0,
            ..arclain_data::CacheLimits::default()
        },
    )
    .expect("create content cache");
    (directory, cache, index)
}

fn indexed_entry(key: &str, content_hash: &str, size_bytes: Option<i64>) -> CacheEntry {
    CacheEntry {
        id: 1,
        key: key.to_string(),
        product_id: None,
        content_hash: content_hash.to_string(),
        source_url: None,
        cache_type: CacheType::Other,
        created_at: String::new(),
        last_accessed: None,
        size_bytes,
    }
}

#[test]
fn content_resolver_uses_request_plugin_owner_for_reads_and_writes() {
    let (_directory, cache, _index) = test_cache();
    let manager = Arc::new(ResourceManager::new(
        Arc::new(cache),
        ResourceConfig::default(),
    ));
    let resolver = ContentCacheResolver::new(manager);
    let plugin_a = DataRequest::new("shared").with_plugin_id("plugin-a");
    let plugin_b = DataRequest::new("shared").with_plugin_id("plugin-b");
    let host = DataRequest::new("shared");

    resolver.try_store("shared", b"private", &plugin_a).unwrap();

    assert_eq!(
        resolver.try_resolve("shared", &plugin_a).unwrap(),
        b"private"
    );
    assert!(resolver.try_resolve("shared", &plugin_b).is_err());
    assert!(resolver.try_resolve("shared", &host).is_err());
}

#[test]
fn bounded_cache_read_rejects_declared_oversize_before_opening_content() {
    const LIMIT: usize = 8;
    let (_directory, cache, index) = test_cache();
    index.set_entry(indexed_entry(
        "declared-large",
        "not-a-valid-integrity",
        Some((LIMIT + 1) as i64),
    ));

    let error = cache
        .get_with_limit("declared-large", LIMIT)
        .expect_err("declared oversized cache entry should be rejected");

    assert!(error.to_string().contains("exceeds"));
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 0);
}

#[test]
fn bounded_cache_read_rejects_actual_oversize_when_metadata_is_stale() {
    const LIMIT: usize = 8;
    let (_directory, cache, index) = test_cache();
    cache
        .put("stale", b"123456789", CacheType::Other, None, None)
        .expect("seed stale-sized cache entry");
    index.set_size(Some(1));

    let error = cache
        .get_with_limit("stale", LIMIT)
        .expect_err("actual oversized cache entry should be rejected");

    assert!(error.to_string().contains("exceeds"));
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 0);
}

#[test]
fn bounded_cache_read_enforces_limit_when_metadata_is_missing() {
    const LIMIT: usize = 8;
    let (_directory, cache, index) = test_cache();
    cache
        .put("missing", b"123456789", CacheType::Other, None, None)
        .expect("seed cache entry without size metadata");
    index.set_size(None);

    let error = cache
        .get_with_limit("missing", LIMIT)
        .expect_err("unknown-sized oversized cache entry should be rejected");

    assert!(error.to_string().contains("exceeds"));
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 0);
}

#[test]
fn bounded_cache_read_accepts_exact_limit_with_missing_metadata() {
    const LIMIT: usize = 8;
    let (_directory, cache, index) = test_cache();
    cache
        .put("boundary", b"12345678", CacheType::Other, None, None)
        .expect("seed boundary cache entry");
    index.set_size(None);

    let body = cache
        .get_with_limit("boundary", LIMIT)
        .expect("read boundary cache entry")
        .expect("boundary cache entry should exist");

    assert_eq!(body, b"12345678");
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 1);
}

#[test]
fn configured_limit_reaches_resource_manager_and_data_service_cache_reads() {
    const LIMIT: usize = 8;
    let (_directory, cache, index) = test_cache();
    cache
        .put("large", b"123456789", CacheType::Other, None, None)
        .expect("seed oversized cache entry");
    let manager = Arc::new(ResourceManager::new(
        Arc::new(cache),
        ResourceConfig {
            max_resource_size: Some(LIMIT),
            ..ResourceConfig::default()
        },
    ));

    assert!(manager.get("large").is_none());

    let service = DataService::new();
    service.register_resolver(
        DataSource::ContentCache,
        Arc::new(ContentCacheResolver::new(manager)),
    );
    assert!(service.get_data("large").is_none());

    let request_id =
        service.request_data(DataRequest::new("large").with_sources([DataSource::ContentCache]));
    let result = service.poll_data(&request_id);
    assert_eq!(result.status, DataStatus::Failed);
    assert!(result.data.is_none());
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 0);
}

#[test]
fn resource_manager_rechecks_memory_entries_against_the_current_read_limit() {
    const LIMIT: usize = 8;
    let mut manager = ResourceManager::without_cache(ResourceConfig {
        default_strategy: StorageStrategy::Memory,
        max_resource_size: Some(LIMIT + 1),
        ..ResourceConfig::default()
    });
    manager
        .put(
            "memory-large",
            b"123456789",
            &ResourceRequest::from_url("memory-large", "https://example.invalid"),
        )
        .expect("seed memory resource");
    manager.set_config(ResourceConfig {
        default_strategy: StorageStrategy::Memory,
        max_resource_size: Some(LIMIT),
        ..ResourceConfig::default()
    });

    assert!(manager.get("memory-large").is_none());
}

#[test]
fn resource_manager_bounds_fallback_file_reads() {
    const LIMIT: usize = 8;
    let directory = tempfile::tempdir().expect("create fallback tempdir");
    fs::write(directory.path().join("disk-large"), b"123456789")
        .expect("seed oversized fallback file");
    let manager = ResourceManager::without_cache(ResourceConfig {
        fallback_dir: Some(directory.path().to_path_buf()),
        max_resource_size: Some(LIMIT),
        ..ResourceConfig::default()
    });

    assert!(manager.get("disk-large").is_none());
}

#[test]
fn resource_manager_accepts_fallback_file_at_exact_limit() {
    const LIMIT: usize = 8;
    let directory = tempfile::tempdir().expect("create fallback tempdir");
    fs::write(directory.path().join("disk-boundary"), b"12345678")
        .expect("seed boundary fallback file");
    let manager = ResourceManager::without_cache(ResourceConfig {
        fallback_dir: Some(directory.path().to_path_buf()),
        max_resource_size: Some(LIMIT),
        ..ResourceConfig::default()
    });

    let resource = manager
        .get("disk-boundary")
        .expect("exact-limit fallback resource should be readable");
    assert_eq!(resource.data, b"12345678");
}

#[test]
fn corrupted_cache_content_is_rejected_without_access_update() {
    const LIMIT: usize = 64;
    let (_directory, cache, index) = test_cache();
    let sri_text = cache
        .put("corrupted", b"known-good", CacheType::Other, None, None)
        .expect("seed cache entry");
    let sri: ssri::Integrity = sri_text.parse().expect("parse seeded integrity");
    let (algorithm, hex) = sri.to_hex();
    let content_path = cache
        .base_dir()
        .join("content-v2")
        .join(algorithm.to_string())
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(&hex[4..]);
    fs::write(content_path, b"tampered!!").expect("corrupt cached body in place");

    let error = cache
        .get_with_limit("corrupted", LIMIT)
        .expect_err("integrity mismatch must reject cached bytes");

    assert!(error.to_string().to_ascii_lowercase().contains("integrity"));
    assert_eq!(index.access_updates.load(Ordering::SeqCst), 0);
}
