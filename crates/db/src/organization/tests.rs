//! Unit tests for organization module

use super::rules::*;
use rusqlite::Connection;

fn setup_org_db() -> Connection {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");

    conn.execute_batch(
        r#"
        CREATE TABLE organization_rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            category TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            is_enabled INTEGER NOT NULL DEFAULT 1,
            is_system INTEGER NOT NULL DEFAULT 0,
            trigger_json TEXT NOT NULL,
            actions_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            modified_at TEXT
        );
        "#,
    )
    .expect("Failed to create test schema");

    conn
}

fn sample_rule(name: &str) -> DbOrganizationRule {
    DbOrganizationRule {
        id: None, // Will be auto-assigned
        name: name.to_string(),
        description: Some("Test description".to_string()),
        category: "test".to_string(),
        priority: 10,
        is_enabled: true,
        is_system: false,
        trigger_json: r#"{"type":"extension","value":"zip"}"#.to_string(),
        actions_json: r#"[{"type":"move","target":"archives"}]"#.to_string(),
    }
}

#[test]
fn test_save_and_get_rule() {
    let conn = setup_org_db();

    let rule = sample_rule("Test Rule");
    let id = save_rule(&conn, &rule).expect("Failed to save rule");

    let loaded = get_rule(&conn, id)
        .expect("Failed to get rule")
        .expect("Rule not found");

    assert_eq!(loaded.name, "Test Rule");
    assert_eq!(loaded.category, "test");
    assert_eq!(loaded.priority, 10);
}

#[test]
fn test_list_rules() {
    let conn = setup_org_db();

    save_rule(&conn, &sample_rule("Rule A")).unwrap();
    save_rule(&conn, &sample_rule("Rule B")).unwrap();
    save_rule(&conn, &sample_rule("Rule C")).unwrap();

    let rules = list_rules(&conn).expect("Failed to list rules");
    assert_eq!(rules.len(), 3);
}

#[test]
fn test_delete_rule() {
    let conn = setup_org_db();

    let id = save_rule(&conn, &sample_rule("DeleteMe")).unwrap();
    assert!(get_rule(&conn, id).unwrap().is_some());

    delete_rule(&conn, id).expect("Failed to delete rule");

    assert!(get_rule(&conn, id).unwrap().is_none());
}

#[test]
fn test_update_rule() {
    let conn = setup_org_db();

    let mut rule = sample_rule("UpdateMe");
    let id = save_rule(&conn, &rule).unwrap();

    // Update the rule
    rule.id = Some(id);
    rule.priority = 100;
    rule.description = Some("Updated description".to_string());
    save_rule(&conn, &rule).unwrap();

    let loaded = get_rule(&conn, id).unwrap().unwrap();
    assert_eq!(loaded.priority, 100);
    assert_eq!(loaded.description, Some("Updated description".to_string()));
}
