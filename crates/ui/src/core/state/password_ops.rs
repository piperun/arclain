//! Password rules management operations

use super::AppState;
use anyhow::Result;
use arclain_core::utilities::PassRule;
use arclain_core::DbPassRule;
use std::path::Path;
use tracing::{info, warn};

impl AppState {
    /// Save password rules to the encrypted secrets database
    pub fn save_password_rules(&mut self, rules: Vec<PassRule>) -> Result<()> {
        // Update in-memory cache
        self.pass_rules = rules.clone();

        // Persist to secrets DB if available
        if let Some(ref dbs) = self.dbs {
            let db_rules: Vec<DbPassRule> = rules
                .into_iter()
                .map(|r| DbPassRule {
                    name: r.name,
                    pattern: r.pattern,
                    password: r.password,
                    priority: r.priority,
                    enabled: r.enabled,
                })
                .collect();
            if let Err(e) = dbs.secrets.replace_all_pass_rules(&db_rules) {
                warn!("Failed to save password rules to DB: {}", e);
            } else {
                info!(
                    "Saved {} password rules to encrypted secrets DB",
                    db_rules.len()
                );
            }
        } else {
            warn!("Cannot save password rules - DB not available (rules updated in memory only)");
        }

        Ok(())
    }

    pub fn save_password_rule_from_archive(
        &mut self,
        archive_path: &Path,
        password: &str,
    ) -> Result<()> {
        let filename = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if filename.is_empty() {
            return Ok(());
        }

        // Generate a pattern from the filename
        let pattern = regex::escape(filename);

        let new_rule = PassRule {
            name: format!("Auto-saved: {}", filename),
            pattern,
            password: password.to_string(),
            priority: 10,
            enabled: true,
        };

        let mut rules = self.pass_rules.clone();
        // Check if a rule with this pattern already exists
        if let Some(existing) = rules.iter_mut().find(|r| r.pattern == new_rule.pattern) {
            existing.password = new_rule.password.clone();
            existing.enabled = true;
        } else {
            rules.push(new_rule);
        }

        self.save_password_rules(rules)
    }
}
