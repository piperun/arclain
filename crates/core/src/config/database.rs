// Thin wrapper around arclain_db for UI/core stability
use anyhow::Result;
pub use arclain_db::{
    get_config, open_databases, set_config, ConfigDb, ConfigDbs, DbPaths, DieselPool, SecretsDb,
    SecretsKey,
};

use crate::config::PassRule;
use arclain_db::DbPassRule;

/// List pass rules from encrypted DB, mapped into core::config::PassRule
pub fn list_pass_rules(db: &arclain_db::SecretsDb) -> Result<Vec<PassRule>> {
    let rules = db.list_pass_rules()?;
    Ok(rules
        .into_iter()
        .map(|r| PassRule {
            name: r.name,
            pattern: r.pattern,
            password: r.password,
            priority: r.priority,
            enabled: r.enabled,
        })
        .collect())
}

/// Replace all pass rules in encrypted DB from core::config::PassRule list
pub fn replace_pass_rules(db: &arclain_db::SecretsDb, rules: &[PassRule]) -> Result<()> {
    let mapped: Vec<DbPassRule> = rules
        .iter()
        .map(|r| DbPassRule {
            name: r.name.clone(),
            pattern: r.pattern.clone(),
            password: r.password.clone(),
            priority: r.priority,
            enabled: r.enabled,
        })
        .collect();
    db.replace_all_pass_rules(&mapped)
}

// Organization Rules

use crate::features::organization::OrganizationRule;
use arclain_db::{
    delete_rule, delete_title_replacement, list_rules, list_title_replacements, save_rule,
    save_title_replacement, DbOrganizationRule,
};

pub fn list_org_rules(pool: &DieselPool) -> Result<Vec<OrganizationRule>> {
    pool.with_conn(|conn| {
        let db_rules = list_rules(conn)?;
        let mut rules = Vec::new();

        for r in db_rules {
            rules.push(OrganizationRule {
                id: r.id.unwrap_or(0) as i64,
                name: r.name,
                priority: r.priority,
                is_enabled: r.is_enabled,
                trigger: serde_json::from_str(&r.trigger_json).unwrap_or_default(),
                actions: serde_json::from_str(&r.actions_json).unwrap_or_default(),
            });
        }

        Ok(rules)
    })
}

pub fn save_org_rule(pool: &DieselPool, rule: &OrganizationRule) -> Result<i64> {
    pool.with_conn(|conn| {
        // Retrieve existing rule by name to get ID if possible
        // This is a bit inefficient but safe for this refactor scope.
        let existing_rules = list_rules(conn)?;
        let existing_id = existing_rules
            .iter()
            .find(|r| r.name == rule.name)
            .map(|r| r.id);

        let db_rule = DbOrganizationRule {
            id: existing_id.flatten(),
            name: rule.name.clone(),
            description: None,
            category: "General".to_string(),
            priority: rule.priority,
            is_enabled: rule.is_enabled,
            is_system: false,
            trigger_json: serde_json::to_string(&rule.trigger).unwrap_or_default(),
            actions_json: serde_json::to_string(&rule.actions).unwrap_or_default(),
        };
        save_rule(conn, &db_rule)
    })
}

pub fn delete_org_rule(pool: &DieselPool, id: i64) -> Result<()> {
    pool.with_conn(|conn| delete_rule(conn, id as i32))
}

// Title Replacements

pub use arclain_db::DbTitleReplacement;

pub fn list_replacements(pool: &DieselPool) -> Result<Vec<DbTitleReplacement>> {
    pool.with_conn(|conn| list_title_replacements(conn))
}

pub fn save_replacement(
    pool: &DieselPool,
    original: &str,
    replacement: &str,
    is_system: bool,
) -> Result<()> {
    pool.with_conn(|conn| save_title_replacement(conn, original, replacement, is_system))
}

pub fn delete_replacement(pool: &DieselPool, id: i64) -> Result<()> {
    pool.with_conn(|conn| delete_title_replacement(conn, id as i32))
}

/// Ensure default rules exist in the database
pub fn ensure_default_rules(pool: &DieselPool) -> Result<()> {
    let rules = list_org_rules(pool)?;
    if !rules.is_empty() {
        return Ok(());
    }

    // Seed default DLsite rule
    let dlsite_rule = OrganizationRule {
        name: "DLsite Standard".to_string(),
        priority: 100,
        is_enabled: true,
        trigger: crate::features::organization::RuleTrigger {
            filename_pattern: Some(r"(RJ|VJ|BJ)\d+".to_string()),
            has_file: None,
            metadata_source: None,
        },
        actions: crate::features::organization::RuleActions {
            root_folder: Some("Game".to_string()),
            use_standard_layout: true,
            move_files: vec![],
            ..Default::default()
        },
        ..Default::default()
    };

    save_org_rule(pool, &dlsite_rule)?;
    tracing::info!("Seeded default DLsite rule");

    Ok(())
}

/// Upsert system rules (e.g. from plugins)
/// Matches existing system rules by name and updates them.
/// Creates new rules if not found.
pub fn upsert_system_rules(pool: &DieselPool, rules: &[OrganizationRule]) -> Result<()> {
    let existing_rules = list_org_rules(pool)?;

    for rule in rules {
        // Find existing rules by name
        if let Some(_) = existing_rules.iter().find(|r| r.name == rule.name) {
            save_org_rule(pool, rule)?;
            tracing::debug!("Updated system rule: {}", rule.name);
        } else {
            save_org_rule(pool, rule)?;
            tracing::info!("Inserted new system rule: {}", rule.name);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_org_rules_table(pool: &DieselPool) {
        pool.with_conn(|conn| {
            // Manually create table to avoid complex migration setup in unit test
            use diesel::connection::SimpleConnection;
            conn.batch_execute(
                "CREATE TABLE organization_rules (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT,
                    category TEXT NOT NULL,
                    trigger_json TEXT NOT NULL,
                    actions_json TEXT NOT NULL,
                    priority INTEGER NOT NULL,
                    is_enabled BOOLEAN NOT NULL,
                    is_system BOOLEAN NOT NULL,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    modified_at TEXT
                );",
            )
            .expect("Failed to create table");
            Ok::<(), anyhow::Error>(())
        })
        .expect("Failed to execute setup");
    }

    #[test]
    fn test_ensure_default_rules() {
        // Setup temp DB
        let temp_file = NamedTempFile::new().unwrap();
        let pool = DieselPool::new(temp_file.path()).unwrap();

        // Setup schema
        setup_org_rules_table(&pool);

        // 1. Ensure empty initially
        let rules = list_org_rules(&pool).unwrap();
        assert!(rules.is_empty());

        // 2. Run ensure_default_rules
        ensure_default_rules(&pool).unwrap();

        // 3. Verify rules exist
        let rules = list_org_rules(&pool).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "DLsite Standard");

        // 4. Run again, should not duplicate
        ensure_default_rules(&pool).unwrap();
        let rules = list_org_rules(&pool).unwrap();
        assert_eq!(rules.len(), 1);
    }
}
