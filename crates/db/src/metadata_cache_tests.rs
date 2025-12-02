use crate::*;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn setup_test_db() -> Arc<Mutex<DbConnection>> {
    let conn = DbConnection::open_in_memory().expect("Failed to open in-memory DB");
    // Initialize schema manually since init_config_schema might not cover this table if it's new
    // But wait, init_config_schema IS where the table is created.
    // Let's assume we can call init_config_schema or just create the table manually for isolation.

    conn.execute(
        "CREATE TABLE IF NOT EXISTS dlsite_metadata_cache (
            product_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            circle TEXT,
            price INTEGER,
            release_date TEXT,
            metadata_json TEXT NOT NULL,
            cached_at INTEGER NOT NULL
        )",
        [],
    )
    .expect("Failed to create table");

    Arc::new(Mutex::new(conn))
}

#[test]
fn test_metadata_cache_crud() {
    let conn = setup_test_db();
    let cache = MetadataCache::new(conn);

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
    let conn = setup_test_db();
    let cache = MetadataCache::new(conn);

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
    let conn = setup_test_db();
    let cache = MetadataCache::new(conn);

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
