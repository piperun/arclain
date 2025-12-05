use crate::config::database::{list_org_rules, save_org_rule};
use crate::organization::OrganizationRule;
use anyhow::Result;
use arclain_db::SqliteDb;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct RulesFile {
    rules: Vec<OrganizationRule>,
}

/// Synchronize organization rules from TOML files to the database
/// Only runs if the database is empty
pub fn sync_rules(db: &SqliteDb) -> Result<()> {
    // Check if we have any rules in DB
    let existing_rules = list_org_rules(db)?;
    if !existing_rules.is_empty() {
        return Ok(());
    }

    tracing::info!("Organization rules DB is empty. Syncing from defaults...");

    // Ensure assets/rules directory exists
    let rules_dir = Path::new("assets/rules");
    if !rules_dir.exists() {
        std::fs::create_dir_all(rules_dir)?;
    }

    // Check for defaults.toml, create if missing
    let defaults_path = rules_dir.join("defaults.toml");
    if !defaults_path.exists() {
        // We should have created this via the artifact, but just in case
        tracing::warn!("defaults.toml missing, skipping sync");
        return Ok(());
    }

    // Read all TOML files in directory
    for entry in std::fs::read_dir(rules_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let content = std::fs::read_to_string(&path)?;
            match toml::from_str::<RulesFile>(&content) {
                Ok(file) => {
                    for rule in file.rules {
                        save_org_rule(db, &rule)?;
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to parse rules file {:?}: {}", path, e);
                }
            }
        }
    }

    Ok(())
}
