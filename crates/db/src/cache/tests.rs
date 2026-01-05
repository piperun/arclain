//! Unit tests for cache module

use super::*;

fn setup_test_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Create the referenced table first
    conn.execute(
        "CREATE TABLE dlsite_metadata_cache (product_id TEXT PRIMARY KEY)",
        [],
    )
    .unwrap();
    init_cache_index_schema(&conn).unwrap();
    conn
}

#[test]
fn test_upsert_and_get() {
    let conn = setup_test_db();

    // Insert parent record first (for FK constraint)
    conn.execute(
        "INSERT INTO dlsite_metadata_cache (product_id) VALUES (?1)",
        ["RJ123456"],
    )
    .unwrap();

    upsert_cache_entry(
        &conn,
        "dlsite:RJ123456:screenshot_0",
        Some("RJ123456"),
        "sha256-abc123",
        Some("https://example.com/img.jpg"),
        CacheType::Screenshot,
        Some(1024),
    )
    .unwrap();

    let entry = get_cache_entry(&conn, "dlsite:RJ123456:screenshot_0")
        .unwrap()
        .unwrap();

    assert_eq!(entry.key, "dlsite:RJ123456:screenshot_0");
    assert_eq!(entry.product_id, Some("RJ123456".to_string()));
    assert_eq!(entry.content_hash, "sha256-abc123");
    assert_eq!(entry.cache_type, CacheType::Screenshot);
}

#[test]
fn test_has_entry() {
    let conn = setup_test_db();

    assert!(!has_cache_entry(&conn, "nonexistent").unwrap());

    upsert_cache_entry(
        &conn,
        "test_key",
        None,
        "hash123",
        None,
        CacheType::Other,
        None,
    )
    .unwrap();

    assert!(has_cache_entry(&conn, "test_key").unwrap());
}
