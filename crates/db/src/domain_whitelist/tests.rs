//! Unit tests for domain_whitelist module

use super::*;
use rusqlite::Connection;

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_whitelist_table(&conn).unwrap();
    conn
}

#[test]
fn test_add_and_list() {
    let conn = setup_db();

    let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
    upsert_whitelist_entry(&conn, &entry).unwrap();

    let entries = list_whitelist_entries(&conn).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].plugin_id, "test-plugin");
    assert_eq!(entries[0].domain, "dlsite.com");
    assert!(!entries[0].approved);
}

#[test]
fn test_approve() {
    let conn = setup_db();

    let entry = DbWhitelistEntry::pending("test-plugin", "dlsite.com");
    upsert_whitelist_entry(&conn, &entry).unwrap();

    assert!(!is_domain_approved(&conn, "test-plugin", "dlsite.com").unwrap());

    approve_domain(&conn, "test-plugin", "dlsite.com").unwrap();

    assert!(is_domain_approved(&conn, "test-plugin", "dlsite.com").unwrap());
}

#[test]
fn test_revoke() {
    let conn = setup_db();

    approve_domain(&conn, "test-plugin", "example.com").unwrap();
    assert!(is_domain_approved(&conn, "test-plugin", "example.com").unwrap());

    revoke_domain(&conn, "test-plugin", "example.com").unwrap();
    assert!(!is_domain_approved(&conn, "test-plugin", "example.com").unwrap());
}

#[test]
fn test_pending_list() {
    let conn = setup_db();

    upsert_whitelist_entry(&conn, &DbWhitelistEntry::pending("p1", "a.com")).unwrap();
    upsert_whitelist_entry(&conn, &DbWhitelistEntry::approved("p1", "b.com")).unwrap();
    upsert_whitelist_entry(&conn, &DbWhitelistEntry::pending("p2", "c.com")).unwrap();

    let pending = list_pending_approvals(&conn).unwrap();
    assert_eq!(pending.len(), 2);
}
