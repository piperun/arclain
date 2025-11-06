// Thin wrapper around arclain_db for UI/core stability
use anyhow::Result;
pub use arclain_db::{
    get_config, open_config_db, open_databases, set_config, ConfigDbs, DbPaths, SecretsDb,
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
