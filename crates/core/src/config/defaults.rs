use crate::organization::{OrganizationRule, RuleActions, RuleTrigger};

pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![OrganizationRule {
        name: "DLSite Archive".to_string(),
        priority: 100,
        is_enabled: true,
        trigger: RuleTrigger {
            metadata_source: Some("dlsite".to_string()),
            filename_pattern: Some(r"\[(RJ|BJ|VJ)\d+\]".to_string()),
            has_file: None,
        },
        actions: RuleActions {
            root_folder: Some("[$product_id][$circle] $title".to_string()),
            use_standard_layout: true,
            move_files: vec![],
        },
    }]
}
