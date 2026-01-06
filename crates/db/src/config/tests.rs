//! Unit tests for config module

use super::config_db::*;
use rusqlite::Connection;

fn setup_config_db() -> Connection {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");

    // Create minimal schema for testing
    conn.execute_batch(
        r#"
        CREATE TABLE app_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        
        CREATE TABLE title_replacements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            original TEXT NOT NULL UNIQUE,
            replacement TEXT NOT NULL,
            is_system INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        "#,
    )
    .expect("Failed to create test schema");

    conn
}

#[test]
fn test_title_replacement_save_and_list() {
    let conn = setup_config_db();

    // Save a replacement
    save_title_replacement(&conn, "Original Text", "Replaced Text", false)
        .expect("Failed to save replacement");

    // List replacements
    let list = list_title_replacements(&conn).expect("Failed to list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].original, "Original Text");
    assert_eq!(list[0].replacement, "Replaced Text");
}

#[test]
fn test_title_replacement_delete() {
    let conn = setup_config_db();

    save_title_replacement(&conn, "ToDelete", "Replacement", false).unwrap();

    let list = list_title_replacements(&conn).unwrap();
    assert_eq!(list.len(), 1);

    delete_title_replacement(&conn, list[0].id as i64).expect("Failed to delete");

    let list = list_title_replacements(&conn).unwrap();
    assert!(list.is_empty());
}

#[test]
fn test_title_replacement_upsert() {
    let conn = setup_config_db();

    save_title_replacement(&conn, "Orig", "Repl1", false).unwrap();

    // Saving same original should update
    save_title_replacement(&conn, "Orig", "Repl2", false).unwrap();

    let list = list_title_replacements(&conn).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].replacement, "Repl2");
}
