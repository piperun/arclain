//! Password rules management operations

use super::AppState;
use anyhow::Result;
use arclain_core::utilities::password_matcher::derive_pattern_for;
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

        // Derive the broadest reasonable pattern from the filename.
        // See `derive_pattern_for` for the heuristic — RJ/VJ/BJ code
        // first, then leading [Maker] bracket, then literal-filename
        // fallback. The old behavior used regex::escape(filename)
        // unconditionally, which produced a rule matching exactly
        // one archive — bad for the common case where a user has
        // multiple archives from the same source sharing a password.
        let pattern = derive_pattern_for(filename);

        info!(
            "Auto-saving password rule for archive '{}' with pattern '{}'",
            filename, pattern
        );

        let new_rule = PassRule {
            name: format!("Auto-saved: {}", filename),
            pattern,
            password: password.to_string(),
            priority: 10,
            enabled: true,
        };

        let mut rules = self.pass_rules.clone();
        // Check if a rule with this pattern already exists. With
        // the broader heuristic, a second archive in the same
        // RJ-code-or-bracket family will now collide with an
        // existing rule instead of creating a redundant duplicate —
        // we just refresh the password (defensive) and re-enable
        // (in case the user manually disabled it earlier).
        if let Some(existing) = rules.iter_mut().find(|r| r.pattern == new_rule.pattern) {
            existing.password = new_rule.password.clone();
            existing.enabled = true;
        } else {
            rules.push(new_rule);
        }

        self.save_password_rules(rules)
    }
}
