use super::{MoveFileRule, MoveRule, OrganizationRule, RuleActions, RuleTrigger};

pub fn get_default_rules() -> Vec<OrganizationRule> {
    vec![OrganizationRule {
        id: Some(-1), // System rule ID
        name: "DLSite Archive".to_string(),
        description: Some("Organizes DLSite archives (RJ codes)".to_string()),
        category: "Doujin".to_string(),
        priority: 100,
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
            root_folder: Some("[$circle] $title [$code]".to_string()),
            move_files: vec![
                // Move executables and game data to Game/
                MoveFileRule {
                    pattern: "*.exe".to_string(),
                    target: "Game".to_string(),
                },
                MoveFileRule {
                    pattern: "*.dll".to_string(),
                    target: "Game".to_string(),
                },
                MoveFileRule {
                    pattern: "*_Data".to_string(), // Unity data folders
                    target: "Game".to_string(),
                },
                // Move images to Images/
                MoveFileRule {
                    pattern: "*.jpg".to_string(),
                    target: "Images".to_string(),
                },
                MoveFileRule {
                    pattern: "*.png".to_string(),
                    target: "Images".to_string(),
                },
                // Catch-all: everything else to Game/
                MoveFileRule {
                    pattern: "**".to_string(),
                    target: "Game".to_string(),
                },
            ],
            move_to: Some(MoveRule {
                target_dir: "DLSite".to_string(),
                use_date: false,
                use_category: false,
            }),
            rename_pattern: None,
            organize_content: true,
            delete_original: false,
        },
    }]
}
