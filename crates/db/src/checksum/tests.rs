//! Unit tests for checksum module

use super::checksum_db::*;
use rusqlite::Connection;
use std::path::PathBuf;

fn setup_checksum_db() -> Connection {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");

    // Create schema manually for testing
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS checksum_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO checksum_settings (key, value) VALUES ('algorithm', 'crc32');
        INSERT OR IGNORE INTO checksum_settings (key, value) VALUES ('mode', 'simple');

        CREATE TABLE IF NOT EXISTS file_checksums (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            archive_id TEXT,
            hash BLOB NOT NULL,
            size INTEGER NOT NULL,
            mtime INTEGER,
            algorithm TEXT NOT NULL,
            computed_at INTEGER NOT NULL,
            UNIQUE(path, archive_id)
        );

        CREATE TABLE IF NOT EXISTS merkle_roots (
            id INTEGER PRIMARY KEY,
            archive_id TEXT UNIQUE,
            root_hash BLOB NOT NULL,
            file_count INTEGER NOT NULL,
            algorithm TEXT NOT NULL,
            computed_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS checksum_operations (
            op_id TEXT PRIMARY KEY,
            op_type TEXT NOT NULL,
            state TEXT NOT NULL,
            source_path TEXT NOT NULL,
            dest_path TEXT,
            source_hash BLOB,
            dest_hash BLOB,
            error_message TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )
    .expect("Failed to create test schema");

    conn
}

#[test]
fn test_get_set_algorithm() {
    let conn = setup_checksum_db();

    let algo = get_checksum_algorithm(&conn).expect("Failed to get algorithm");
    assert_eq!(algo, "crc32"); // default

    set_checksum_algorithm(&conn, "blake3").expect("Failed to set algorithm");

    let algo = get_checksum_algorithm(&conn).unwrap();
    assert_eq!(algo, "blake3");
}

#[test]
fn test_get_set_mode() {
    let conn = setup_checksum_db();

    let mode = get_checksum_mode(&conn).expect("Failed to get mode");
    assert_eq!(mode, VerifyMode::Simple); // default

    set_checksum_mode(&conn, VerifyMode::Full).expect("Failed to set mode");

    let mode = get_checksum_mode(&conn).unwrap();
    assert_eq!(mode, VerifyMode::Full);
}

#[test]
fn test_store_and_get_file_checksum() {
    let conn = setup_checksum_db();

    let hash = vec![1, 2, 3, 4, 5, 6, 7, 8];
    store_file_checksum(&conn, "/path/to/file.txt", None, &hash, 1024, "crc32")
        .expect("Failed to store checksum");

    let result = get_file_checksum(&conn, "/path/to/file.txt", None)
        .expect("Failed to get checksum")
        .expect("Checksum not found");

    assert_eq!(result.path, "/path/to/file.txt");
    assert_eq!(result.hash, hash);
    assert_eq!(result.size, 1024);
    assert_eq!(result.algorithm, "crc32");
}

#[test]
fn test_store_and_get_merkle_root() {
    let conn = setup_checksum_db();

    let root_hash = vec![10, 20, 30, 40];
    store_merkle_root(&conn, "archive123", &root_hash, 100, "blake3")
        .expect("Failed to store merkle root");

    let result = get_merkle_root(&conn, "archive123")
        .expect("Failed to get merkle root")
        .expect("Merkle root not found");

    assert_eq!(result, root_hash);
}

#[test]
fn test_checksum_operation_lifecycle() {
    let conn = setup_checksum_db();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let op = DbOperation {
        id: OpId::from_string("test-op-1".to_string()),
        op_type: OpType::Copy,
        state: OpState::Pending,
        source_path: PathBuf::from("/source/file.txt"),
        dest_path: Some(PathBuf::from("/dest/file.txt")),
        source_hash: None,
        dest_hash: None,
        error_message: None,
        created_at: now,
        updated_at: now,
    };

    // Begin operation
    begin_checksum_operation(&conn, &op).expect("Failed to begin operation");

    // Get pending operations
    let pending = get_pending_checksum_operations(&conn).expect("Failed to get pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id.0, "test-op-1");

    // Update operation
    let mut updated_op = op.clone();
    updated_op.state = OpState::Completed;
    updated_op.source_hash = Some(vec![1, 2, 3]);
    update_checksum_operation(&conn, &updated_op).expect("Failed to update");

    // Should no longer be in pending
    let pending = get_pending_checksum_operations(&conn).unwrap();
    assert!(pending.is_empty());

    // Delete operation
    delete_checksum_operation(&conn, &op.id).expect("Failed to delete");
}

#[test]
fn test_verify_mode_conversion() {
    assert_eq!(VerifyMode::from_str("disabled"), Some(VerifyMode::Disabled));
    assert_eq!(VerifyMode::from_str("simple"), Some(VerifyMode::Simple));
    assert_eq!(VerifyMode::from_str("full"), Some(VerifyMode::Full));
    assert_eq!(VerifyMode::from_str("invalid"), None);

    assert_eq!(VerifyMode::Disabled.as_str(), "disabled");
    assert_eq!(VerifyMode::Simple.as_str(), "simple");
    assert_eq!(VerifyMode::Full.as_str(), "full");
}
