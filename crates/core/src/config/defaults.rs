use crate::organization::{OrganizationRule, RuleActions, RuleTrigger};

pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![OrganizationRule {
        id: None,
        name: "DLSite Archive".to_string(),
        description: Some("Organizes DLSite archives with metadata (RJ/BJ codes)".to_string()),
        category: "DLSite".to_string(),
        priority: 100,
        is_enabled: true,
        is_system: true,
        trigger: RuleTrigger::default(),
        actions: RuleActions::default(),
    }]
}
