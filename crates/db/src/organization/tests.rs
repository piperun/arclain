//! Unit tests for organization module

use super::rules::*;
use diesel::Connection;
use diesel::RunQueryDsl;

fn setup_org_db() -> diesel::SqliteConnection {
    let mut conn = diesel::SqliteConnection::establish(":memory:")
        .expect("Failed to open in-memory SQLite");

    diesel::sql_query(
        "CREATE TABLE organization_rules (
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
        )",
    )
    .execute(&mut conn)
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
    let mut conn = setup_org_db();

    let rule = sample_rule("Test Rule");
    let id = save_rule(&mut conn, &rule).expect("Failed to save rule");

    let loaded = get_rule(&mut conn, id as i32)
        .expect("Failed to get rule")
        .expect("Rule not found");

    assert_eq!(loaded.name, "Test Rule");
    assert_eq!(loaded.category, "test");
    assert_eq!(loaded.priority, 10);
}

#[test]
fn test_list_rules() {
    let mut conn = setup_org_db();

    save_rule(&mut conn, &sample_rule("Rule A")).unwrap();
    save_rule(&mut conn, &sample_rule("Rule B")).unwrap();
    save_rule(&mut conn, &sample_rule("Rule C")).unwrap();

    let rules = list_rules(&mut conn).expect("Failed to list rules");
    assert_eq!(rules.len(), 3);
}

#[test]
fn test_delete_rule() {
    let mut conn = setup_org_db();

    let id = save_rule(&mut conn, &sample_rule("DeleteMe")).unwrap();
    assert!(get_rule(&mut conn, id as i32).unwrap().is_some());

    delete_rule(&mut conn, id as i32).expect("Failed to delete rule");

    assert!(get_rule(&mut conn, id as i32).unwrap().is_none());
}

#[test]
fn test_update_rule() {
    let mut conn = setup_org_db();

    let mut rule = sample_rule("UpdateMe");
    let id = save_rule(&mut conn, &rule).unwrap();

    // Update the rule
    rule.id = Some(id);
    rule.priority = 100;
    rule.description = Some("Updated description".to_string());
    save_rule(&mut conn, &rule).unwrap();

    let loaded = get_rule(&mut conn, id as i32).unwrap().unwrap();
    assert_eq!(loaded.priority, 100);
    assert_eq!(loaded.description, Some("Updated description".to_string()));
}
