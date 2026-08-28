//! Organization service for rule management
//!
//! Wraps arclain_db organization functions with connection pool management.
//! Provides both raw DB type access (DbOrganizationRule) and domain type access (OrganizationRule).

use anyhow::Result;
use arclain_db::{delete_rule, get_rule, list_rules, save_rule, DbOrganizationRule, DieselPool};

// Re-export domain type for convenience
pub use crate::features::organization::OrganizationRule;

/// Service for managing organization rules
#[derive(Clone)]
pub struct OrganizationService {
    pool: DieselPool,
}

impl OrganizationService {
    /// Create a new organization service with the given connection pool
    pub fn new(pool: DieselPool) -> Self {
        Self { pool }
    }

    // =========================================================================
    // Raw DB type methods (low-level)
    // =========================================================================

    /// List all organization rules (raw DB type)
    pub fn list_rules(&self) -> Result<Vec<DbOrganizationRule>> {
        self.pool.with_conn(|conn| list_rules(conn))
    }

    /// Get a specific rule by ID (raw DB type)
    pub fn get_rule(&self, rule_id: i32) -> Result<Option<DbOrganizationRule>> {
        self.pool.with_conn(|conn| get_rule(conn, rule_id))
    }

    /// Save a rule (raw DB type, insert or update)
    pub fn save_rule(&self, rule: &DbOrganizationRule) -> Result<i64> {
        self.pool.with_conn(|conn| save_rule(conn, rule))
    }

    /// Delete a rule by ID (only non-system rules)
    pub fn delete_rule(&self, rule_id: i32) -> Result<()> {
        self.pool.with_conn(|conn| delete_rule(conn, rule_id))
    }

    /// List enabled rules only (raw DB type)
    pub fn list_enabled_rules(&self) -> Result<Vec<DbOrganizationRule>> {
        let all = self.list_rules()?;
        Ok(all.into_iter().filter(|r| r.is_enabled).collect())
    }

    // =========================================================================
    // Domain type methods (high-level, with JSON serialization)
    // =========================================================================

    /// List all rules as domain type (OrganizationRule)
    pub fn list_domain_rules(&self) -> Result<Vec<OrganizationRule>> {
        self.pool.with_conn(|conn| {
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

    /// Get a domain rule by ID
    pub fn get_domain_rule(&self, rule_id: i64) -> Result<Option<OrganizationRule>> {
        self.pool
            .with_conn(|conn| match get_rule(conn, rule_id as i32)? {
                Some(r) => Ok(Some(OrganizationRule {
                    id: r.id.unwrap_or(0) as i64,
                    name: r.name,
                    priority: r.priority,
                    is_enabled: r.is_enabled,
                    trigger: serde_json::from_str(&r.trigger_json).unwrap_or_default(),
                    actions: serde_json::from_str(&r.actions_json).unwrap_or_default(),
                })),
                None => Ok(None),
            })
    }

    /// Save a domain rule (with JSON serialization)
    pub fn save_domain_rule(&self, rule: &OrganizationRule) -> Result<i64> {
        self.pool.with_conn(|conn| {
            // Use existing ID from rule if provided, otherwise look up by name
            let rule_id: Option<i64> = if rule.id > 0 {
                Some(rule.id)
            } else {
                let existing_rules = list_rules(conn)?;
                existing_rules
                    .iter()
                    .find(|r| r.name == rule.name)
                    .and_then(|r| r.id)
            };

            let db_rule = DbOrganizationRule {
                id: rule_id,
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

    /// Delete a domain rule by ID
    pub fn delete_domain_rule(&self, rule_id: i64) -> Result<()> {
        self.pool
            .with_conn(|conn| delete_rule(conn, rule_id as i32))
    }
}

impl std::fmt::Debug for OrganizationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrganizationService")
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::layout::{Layout, OutputSelector, Placement, Source};
    use crate::features::organization::{RuleActions, RuleTrigger};

    /// Test that OrganizationRule can be serialized to DbOrganizationRule and back
    #[test]
    fn test_domain_to_db_round_trip() {
        let original = OrganizationRule {
            id: 0,
            name: "Test Rule".to_string(),
            priority: 100,
            is_enabled: true,
            trigger: RuleTrigger::default(),
            actions: RuleActions {
                output_name: None,
                layout: Layout {
                    outputs: OutputSelector::Whole,
                    file_variables: vec![],
                    name: "games/$circle".to_string(),
                    place: vec![Placement {
                        from: Source::Matching("*.exe".to_string()),
                        into: "bin".to_string(),
                    }],
                    generate: vec![],
                    fetch: vec![],
                },
            },
        };

        // Simulate conversion to DB type (what save_domain_rule does)
        let trigger_json = serde_json::to_string(&original.trigger).unwrap();
        let actions_json = serde_json::to_string(&original.actions).unwrap();

        // Simulate conversion back to domain type (what list_domain_rules does)
        let restored_trigger: RuleTrigger = serde_json::from_str(&trigger_json).unwrap();
        let restored_actions: RuleActions = serde_json::from_str(&actions_json).unwrap();

        let restored = OrganizationRule {
            id: original.id,
            name: original.name.clone(),
            priority: original.priority,
            is_enabled: original.is_enabled,
            trigger: restored_trigger,
            actions: restored_actions,
        };

        assert_eq!(original.name, restored.name);
        assert_eq!(original.priority, restored.priority);
        assert_eq!(original.is_enabled, restored.is_enabled);
        assert_eq!(original.actions.layout, restored.actions.layout);
    }

    /// Test that empty/default values serialize correctly
    #[test]
    fn test_empty_rule_serialization() {
        let rule = OrganizationRule::default();

        let trigger_json = serde_json::to_string(&rule.trigger).unwrap();
        let actions_json = serde_json::to_string(&rule.actions).unwrap();

        // Should not panic and should produce valid JSON
        assert!(!trigger_json.is_empty());
        assert!(!actions_json.is_empty());

        // Should deserialize back without error
        let _: RuleTrigger = serde_json::from_str(&trigger_json).unwrap();
        let _: RuleActions = serde_json::from_str(&actions_json).unwrap();
    }
}
