// Thin wrapper around arclain_db for UI/core stability
use anyhow::Result;
pub use arclain_db::{
    get_config, open_databases, set_config, ConfigDb, ConfigDbs, DbPaths, SecretsDb, SecretsKey,
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

use crate::organization::OrganizationRule;
use arclain_db::{delete_rule, list_rules, save_rule, DbOrganizationRule};

pub fn list_org_rules(db: &arclain_db::SqliteDb) -> Result<Vec<OrganizationRule>> {
    db.with_connection(|conn| {
        let db_rules = list_rules(conn)?;
        let mut rules = Vec::new();

        for r in db_rules {
            rules.push(OrganizationRule {
                id: r.id,
                name: r.name,
                description: r.description,
                category: r.category,
                priority: r.priority,
                is_enabled: r.is_enabled,
                is_system: r.is_system,
                trigger: serde_json::from_str(&r.trigger_json).unwrap_or_default(),
                actions: serde_json::from_str(&r.actions_json).unwrap_or_default(),
            });
        }

        Ok(rules)
    })
}

pub fn save_org_rule(db: &arclain_db::SqliteDb, rule: &OrganizationRule) -> Result<i64> {
    db.with_connection(|conn| {
        let db_rule = DbOrganizationRule {
            id: rule.id,
            name: rule.name.clone(),
            description: rule.description.clone(),
            category: rule.category.clone(),
            priority: rule.priority,
            is_enabled: rule.is_enabled,
            is_system: rule.is_system,
            trigger_json: serde_json::to_string(&rule.trigger).unwrap_or_default(),
            actions_json: serde_json::to_string(&rule.actions).unwrap_or_default(),
        };
        save_rule(conn, &db_rule)
    })
}

pub fn delete_org_rule(db: &arclain_db::SqliteDb, id: i64) -> Result<()> {
    db.with_connection(|conn| delete_rule(conn, id))
}

// Title Replacements

pub use arclain_db::{
    delete_title_replacement, list_title_replacements, save_title_replacement, DbTitleReplacement,
};

pub fn list_replacements(db: &arclain_db::SqliteDb) -> Result<Vec<DbTitleReplacement>> {
    db.with_connection(|conn| list_title_replacements(conn))
}

pub fn save_replacement(
    db: &arclain_db::SqliteDb,
    original: &str,
    replacement: &str,
    is_system: bool,
) -> Result<()> {
    db.with_connection(|conn| save_title_replacement(conn, original, replacement, is_system))
}

pub fn delete_replacement(db: &arclain_db::SqliteDb, id: i64) -> Result<()> {
    db.with_connection(|conn| delete_title_replacement(conn, id))
}

/// Ensure default rules exist in the database
pub fn ensure_default_rules(db: &arclain_db::SqliteDb) -> Result<()> {
    let rules = list_org_rules(db)?;
    if !rules.is_empty() {
        return Ok(());
    }

    // Seed default DLsite rule
    let dlsite_rule = OrganizationRule {
        id: None, // Ignored on insert
        name: "DLsite Standard".to_string(),
        description: Some("Standard organization for DLsite works".to_string()),
        category: "dlsite".to_string(),
        priority: 100,
        is_enabled: true,
        is_system: true,
        trigger: crate::organization::RuleTrigger {
            filename_pattern: Some(r"(RJ|VJ|BJ)\d+".to_string()),
            has_file: None,
            extensions: None,
            min_size: None,
            max_size: None,
        },
        actions: crate::organization::RuleActions {
            root_folder: Some("Game".to_string()),
            use_standard_layout: true,
            move_files: vec![],
            move_to: None,
            rename_pattern: None,
            organize_content: true,
            delete_original: false,
        },
    };

    save_org_rule(db, &dlsite_rule)?;
    tracing::info!("Seeded default DLsite rule");

    Ok(())
}
