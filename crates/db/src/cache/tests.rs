//! Unit tests for cache module

use super::*;
use crate::cache::cache_index_rusqlite::{
    get_cache_entry, has_cache_entry, init_cache_index_schema, upsert_cache_entry,
};

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

#[test]
fn upsert_reclassifies_existing_entry_and_product() {
    let conn = setup_test_db();
    conn.execute(
        "INSERT INTO dlsite_metadata_cache (product_id) VALUES (?1), (?2)",
        ["product-a", "product-b"],
    )
    .unwrap();
    upsert_cache_entry(
        &conn,
        "shared-key",
        Some("product-a"),
        "hash-v1",
        None,
        CacheType::Other,
        Some(100),
    )
    .unwrap();

    upsert_cache_entry(
        &conn,
        "shared-key",
        Some("product-b"),
        "hash-v2",
        None,
        CacheType::PluginData,
        Some(200),
    )
    .unwrap();

    let entry = get_cache_entry(&conn, "shared-key").unwrap().unwrap();
    assert_eq!(entry.cache_type, CacheType::PluginData);
    assert_eq!(entry.product_id.as_deref(), Some("product-b"));
}

/// Regression test for C6 from `docs/AUDIT_2026-05-03.md`.
///
/// Pre-fix, `CacheDb::open`'s recovery path used `let _ =` to discard
/// failures from removing `.sqlite-wal` and `.sqlite-shm` after the
/// main DB file was deleted. If the WAL removal failed, the function
/// continued to re-open the DB; SQLite then either consumed the stale
/// WAL (potentially corrupting the new DB) or refused with a
/// confusing error. Either way, the function silently moved on.
///
/// Post-fix, WAL/SHM removal failures abort the recovery so the user
/// can investigate why the old WAL is locked (typically: another
/// arclain process still holds it, or a backup tool has it pinned).
///
/// Trigger by corrupting the main DB to force the recovery path, then
/// holding a Windows file handle on the WAL without `FILE_SHARE_DELETE`
/// so `remove_file` fails. On Unix file removal succeeds despite open
/// handles, so this scenario is Windows-only.
#[cfg(windows)]
#[test]
fn c6_cache_db_open_aborts_when_wal_removal_blocked() {
    use super::cache_db::CacheDb;
    use std::os::windows::fs::OpenOptionsExt;
    use tempfile::TempDir;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    // Deliberately not including FILE_SHARE_DELETE (0x4).

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("cache.sqlite");
    let wal_path = temp.path().join("cache.sqlite-wal");

    // Corrupt the main DB so SqliteDb::open fails and the recovery path runs.
    std::fs::write(&db_path, b"this is not a valid sqlite database").unwrap();

    // Pre-create an EMPTY WAL so the recovery code tries to remove it.
    // An empty WAL would otherwise be tolerated by SQLite on the second
    // open attempt — so without the C6 fix, the function returns Ok and
    // the stale (empty, but unremovable) WAL stays on disk. The bug is
    // the silent failure, regardless of whether SQLite happens to
    // tolerate this particular WAL content.
    std::fs::File::create(&wal_path).unwrap();

    // Hold a non-shareable handle on the WAL to block removal.
    let _wal_handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&wal_path)
        .expect("opening WAL without delete-share");

    let result = CacheDb::open(&db_path);

    assert!(
        result.is_err(),
        "C6 fix regressed: CacheDb::open returned Ok despite WAL removal being blocked. \
         Stale WAL on disk could cause the next open to apply outdated records.",
    );

    // The stale WAL should still be there (we held the handle).
    assert!(
        wal_path.exists(),
        "Sanity: WAL was actually removed; the test fault setup didn't take effect",
    );

    drop(_wal_handle);
}
