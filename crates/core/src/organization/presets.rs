use super::{MoveFileRule, MoveRule, OrganizationRule, RuleActions, RuleTrigger};

pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![
        // Default fallback rule: Simple flattening
        OrganizationRule {
            id: Some(-1), // System rule ID
            name: "Simple Flatten".to_string(),
            description: Some(
                "Default: Flattens nested folders and uses archive name as root".to_string(),
            ),
            category: "General".to_string(),
            priority: 1, // Low priority - fallback
            is_enabled: true,
            is_system: true,
            trigger: RuleTrigger {
                filename_pattern: None, // Matches everything
                min_size: None,
                max_size: None,
                extensions: None,
                has_file: None,
            },
            actions: RuleActions {
                root_folder: None, // Will use archive name automatically
                move_files: vec![
                    // Just move everything to root, flattening structure
                    MoveFileRule {
                        pattern: "**".to_string(),
                        target: ".".to_string(), // Root of organized archive
                    },
                ],
                move_to: None,
                rename_pattern: None,
                organize_content: true,
                delete_original: false,
                use_standard_layout: false,
            },
        },
        // DLSite rule: Only applies when RJ/BJ code is found
        OrganizationRule {
            id: Some(-2), // System rule ID
            name: "DLSite Archive".to_string(),
            description: Some("Organizes DLSite archives with metadata (RJ/BJ codes)".to_string()),
            category: "Doujin".to_string(),
            priority: 100, // High priority - runs first if matched
            is_enabled: true,
            is_system: true,
            trigger: RuleTrigger {
                filename_pattern: Some(r"(RJ|BJ)\d+".to_string()),
                min_size: None,
                max_size: None,
                extensions: None,
                has_file: None,
            },
            actions: RuleActions {
                root_folder: Some("[$code][$circle] $title".to_string()),
                move_files: vec![], // Handled by standard layout
                move_to: Some(MoveRule {
                    target_dir: "DLSite".to_string(),
                    use_date: false,
                    use_category: false,
                }),
                rename_pattern: None,
                organize_content: true,
                delete_original: false,
                use_standard_layout: true,
            },
        },
    ]
}
