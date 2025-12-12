use super::{MoveAction, OrganizationRule, RuleActions, RuleTrigger};

pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![
        // Default fallback rule: Simple flattening
        OrganizationRule {
            name: "Simple Flatten".to_string(),
            priority: 1, // Low priority - fallback
            is_enabled: true,
            trigger: RuleTrigger {
                filename_pattern: None, // Matches everything
                has_file: None,
                metadata_source: None,
            },
            actions: RuleActions {
                root_folder: None, // Will use archive name automatically
                move_files: vec![
                    // Just move everything to root, flattening structure
                    MoveAction {
                        pattern: "**".to_string(),
                        target: ".".to_string(), // Root of organized archive
                    },
                ],
                use_standard_layout: false,
            },
        },
        // DLSite rule: Only applies when RJ/BJ code is found
        OrganizationRule {
            name: "DLSite Archive".to_string(),
            priority: 100, // High priority - runs first if matched
            is_enabled: true,
            trigger: RuleTrigger {
                filename_pattern: Some(r"(RJ|BJ)\d+".to_string()),
                has_file: None,
                metadata_source: None,
            },
            actions: RuleActions {
                root_folder: Some("[$code][$circle] $title".to_string()),
                move_files: vec![], // Handled by standard layout
                use_standard_layout: true,
            },
        },
    ]
}
