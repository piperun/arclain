//! Unit tests for config module

use super::config_db::*;
use diesel::Connection;
use diesel::RunQueryDsl;

fn setup_config_db() -> diesel::SqliteConnection {
    let mut conn = diesel::SqliteConnection::establish(":memory:")
        .expect("Failed to open in-memory SQLite");

    // Create minimal schema for testing
    diesel::sql_query(
        "CREATE TABLE app_config (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .execute(&mut conn)
    .expect("Failed to create app_config");

    diesel::sql_query(
        "CREATE TABLE title_replacements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            original TEXT NOT NULL UNIQUE,
            replacement TEXT NOT NULL,
            is_system INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&mut conn)
    .expect("Failed to create title_replacements");

    conn
}

#[test]
fn test_title_replacement_save_and_list() {
    let mut conn = setup_config_db();

    // Save a replacement
    save_title_replacement(&mut conn, "Original Text", "Replaced Text", false)
        .expect("Failed to save replacement");

    // List replacements
    let list = list_title_replacements(&mut conn).expect("Failed to list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].original, "Original Text");
    assert_eq!(list[0].replacement, "Replaced Text");
}

#[test]
fn test_title_replacement_delete() {
    let mut conn = setup_config_db();

    save_title_replacement(&mut conn, "ToDelete", "Replacement", false).unwrap();

    let list = list_title_replacements(&mut conn).unwrap();
    assert_eq!(list.len(), 1);

    delete_title_replacement(&mut conn, list[0].id).expect("Failed to delete");

    let list = list_title_replacements(&mut conn).unwrap();
    assert!(list.is_empty());
}

#[test]
fn test_title_replacement_upsert() {
    let mut conn = setup_config_db();

    save_title_replacement(&mut conn, "Orig", "Repl1", false).unwrap();

    // Saving same original should update
    save_title_replacement(&mut conn, "Orig", "Repl2", false).unwrap();

    let list = list_title_replacements(&mut conn).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].replacement, "Repl2");
}
