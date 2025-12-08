use crate::config::database::{list_org_rules, save_org_rule};
use anyhow::Result;
use arclain_db::SqliteDb;

/// Synchronize organization rules from defaults to the database
/// Only runs if the database is empty
pub fn sync_rules(db: &SqliteDb) -> Result<()> {
    // Check if we have any rules in DB
    let existing_rules = list_org_rules(db)?;
    tracing::info!("sync_rules: {} existing rules in DB", existing_rules.len());

    if !existing_rules.is_empty() {
        return Ok(());
    }

    tracing::info!("Organization rules DB is empty. Syncing from internal defaults...");

    let defaults = crate::config::defaults::get_default_rules();
    tracing::info!("sync_rules: {} default rules to insert", defaults.len());

    for rule in &defaults {
        tracing::info!(
            "sync_rules: Inserting rule '{}' (category: '{}')",
            rule.name,
            rule.category
        );
        save_org_rule(db, rule)?;
    }

    Ok(())
}
