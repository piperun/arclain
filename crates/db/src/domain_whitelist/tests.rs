//! Unit tests for domain_whitelist module

use super::*;
use diesel::Connection;
use diesel::RunQueryDsl;

fn setup_db() -> diesel::SqliteConnection {
    let mut conn = diesel::SqliteConnection::establish(":memory:")
        .expect("Failed to open in-memory SQLite");

    diesel::sql_query(
        "CREATE TABLE domain_whitelist (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plugin_id TEXT NOT NULL,
            domain TEXT NOT NULL,
            approved INTEGER NOT NULL DEFAULT 0,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP,
            approved_at TEXT,
            UNIQUE(plugin_id, domain)
        )",
    )
    .execute(&mut conn)
    .expect("Failed to create test schema");

    conn
}

#[test]
fn test_add_and_list() {
    let mut conn = setup_db();

    let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
    upsert_whitelist_entry(&mut conn, &entry).unwrap();

    let entries = list_whitelist_entries(&mut conn).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].plugin_id, "test-plugin");
    assert_eq!(entries[0].domain, "dlsite.com");
    assert!(!entries[0].approved);
}

#[test]
fn test_approve() {
    let mut conn = setup_db();

    let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
    upsert_whitelist_entry(&mut conn, &entry).unwrap();

    assert!(!is_domain_approved(&mut conn, "test-plugin", "dlsite.com").unwrap());

    approve_domain(&mut conn, "test-plugin", "dlsite.com").unwrap();

    assert!(is_domain_approved(&mut conn, "test-plugin", "dlsite.com").unwrap());
}

#[test]
fn test_revoke() {
    let mut conn = setup_db();

    approve_domain(&mut conn, "test-plugin", "example.com").unwrap();
    assert!(is_domain_approved(&mut conn, "test-plugin", "example.com").unwrap());

    revoke_domain(&mut conn, "test-plugin", "example.com").unwrap();
    assert!(!is_domain_approved(&mut conn, "test-plugin", "example.com").unwrap());
}

#[test]
fn test_pending_list() {
    let mut conn = setup_db();

    upsert_whitelist_entry(&mut conn, &DbWhitelistEntry::pending("p1", "a.com")).unwrap();
    upsert_whitelist_entry(&mut conn, &DbWhitelistEntry::approved("p1", "b.com")).unwrap();
    upsert_whitelist_entry(&mut conn, &DbWhitelistEntry::pending("p2", "c.com")).unwrap();

    let pending = list_pending_approvals(&mut conn).unwrap();
    assert_eq!(pending.len(), 2);
}
