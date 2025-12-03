use crate::metadata_cache::CachedMetadata;
use crate::*;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

fn setup_test_cache() -> MetadataCache {
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let cache_db = CacheDb::open(temp_file.path()).expect("Failed to open cache db");
    MetadataCache::new(cache_db.into_sqlite_db())
}

#[test]
fn test_metadata_cache_crud() {
    let cache = setup_test_cache();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let meta = CachedMetadata {
        product_id: "RJ123456".to_string(),
        title: "Test Game".to_string(),
        circle: Some("Test Circle".to_string()),
        price: Some(1000),
        release_date: Some("2024-01-01".to_string()),
        metadata_json: "{}".to_string(),
        cached_at: now,
    };

    // Save
    cache.save(&meta).expect("Failed to save metadata");

    // Get
    let retrieved = cache
        .get("RJ123456")
        .expect("Failed to get metadata")
        .unwrap();
    assert_eq!(retrieved.title, "Test Game");
    assert_eq!(retrieved.circle, Some("Test Circle".to_string()));
    assert_eq!(retrieved.price, Some(1000));

    // Get non-existent
    let missing = cache.get("RJ999999").expect("Failed to get missing");
    assert!(missing.is_none());
}

#[test]
fn test_metadata_cache_freshness() {
    let cache = setup_test_cache();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let one_day = 24 * 60 * 60;

    let fresh_meta = CachedMetadata {
        product_id: "RJ_FRESH".to_string(),
        title: "Fresh".to_string(),
        circle: None,
        price: None,
        release_date: None,
        metadata_json: "{}".to_string(),
        cached_at: now,
    };

    let stale_meta = CachedMetadata {
        product_id: "RJ_STALE".to_string(),
        title: "Stale".to_string(),
        circle: None,
        price: None,
        release_date: None,
        metadata_json: "{}".to_string(),
        cached_at: now - (8 * one_day), // 8 days old
    };

    cache.save(&fresh_meta).unwrap();
    cache.save(&stale_meta).unwrap();

    // Check freshness (7 days max age)
    assert!(cache.is_fresh("RJ_FRESH", 7).unwrap());
    assert!(!cache.is_fresh("RJ_STALE", 7).unwrap());
}

#[test]
fn test_metadata_cache_cleanup() {
    let cache = setup_test_cache();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let one_day = 24 * 60 * 60;

    let fresh_meta = CachedMetadata {
        product_id: "RJ_FRESH".to_string(),
        title: "Fresh".to_string(),
        circle: None,
        price: None,
        release_date: None,
        metadata_json: "{}".to_string(),
        cached_at: now,
    };

    let old_meta = CachedMetadata {
        product_id: "RJ_OLD".to_string(),
        title: "Old".to_string(),
        circle: None,
        price: None,
        release_date: None,
        metadata_json: "{}".to_string(),
        cached_at: now - (10 * one_day), // 10 days old
    };

    cache.save(&fresh_meta).unwrap();
    cache.save(&old_meta).unwrap();

    // Clear entries older than 7 days
    let deleted = cache.clear_old(7).unwrap();
    assert_eq!(deleted, 1);

    // Verify
    assert!(cache.get("RJ_FRESH").unwrap().is_some());
    assert!(cache.get("RJ_OLD").unwrap().is_none());
}
