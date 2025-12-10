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
        trigger: RuleTrigger {
            metadata_source: Some("dlsite".to_string()),
            // Also keep regex for backwards compat (if metadata fails to load but filename matches)
            // Or should we strict strict? User said "metadata trigger".
            // Let's keep regex as fallback?
            filename_pattern: Some(r"\[(RJ|BJ|VJ)\d+\]".to_string()),
            ..Default::default()
        },
        actions: RuleActions {
            // Use variables for folder naming: [RJ123456][Circle Name] Game Title
            root_folder: Some("[$product_id][$circle] $title".to_string()),
            use_standard_layout: true, // Now works with preview logic!
            move_files: vec![],
            move_to: None,
            rename_pattern: None,
            organize_content: true,
            delete_original: false,
        },
    }]
}
