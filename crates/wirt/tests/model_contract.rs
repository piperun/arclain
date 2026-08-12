use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use wirt::{
    rules::{MoveFileRule, MoveRule, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger},
    ui_model::{PluginUiNodeDto, PluginUiNodeKind},
    CapabilitiesConfig, PluginAction, PluginInfoConfig, PluginLayout, PluginManifest,
    PluginUiElement, RateLimits, ToastLevel, WirtConfig,
};

fn assert_json_round_trip<T>(value: T)
where
    T: Debug + DeserializeOwned + PartialEq + Serialize,
{
    let json = serde_json::to_string(&value).expect("value should serialize");
    let decoded: T = serde_json::from_str(&json).expect("value should deserialize");
    assert_eq!(decoded, value);
}

#[test]
fn neutral_model_preserves_all_fields_across_json_round_trips() {
    assert_json_round_trip(PluginManifest {
        wirt: WirtConfig {
            abi: "0.3.0".to_string(),
        },
        plugin: PluginInfoConfig {
            id: "sample_plugin".to_string(),
            name: "Sample Plugin".to_string(),
            version: "1.2.3".to_string(),
            author: "Example Author".to_string(),
            description: "A neutral plugin".to_string(),
        },
        capabilities: CapabilitiesConfig {
            network: true,
            network_domains: vec!["example.com".to_string()],
            archive_metadata_read: true,
            archive_metadata_write: false,
            archive_modify: true,
            file_read: true,
            file_write: false,
        },
        rate_limits: RateLimits {
            http_requests_per_minute: 23,
        },
    });

    assert_json_round_trip(PluginLayout::Single {
        elements: vec![PluginUiElement::Label {
            text: "Hello".to_string(),
            bold: true,
            size: Some(18.0),
        }],
    });

    assert_json_round_trip(PluginAction::ShowToast {
        message: "Saved".to_string(),
        level: ToastLevel::Success,
    });

    assert_json_round_trip(PluginUiNodeDto {
        id: "#root".to_string(),
        kind: PluginUiNodeKind::Single {
            children: Vec::new(),
        },
        visible: true,
        enabled: true,
    });

    assert_json_round_trip(PluginRuleDefinition {
        name: "Organize images".to_string(),
        category: "media".to_string(),
        description: Some("Moves image archives".to_string()),
        trigger: PluginRuleTrigger {
            filename_pattern: Some("^images".to_string()),
            has_file: Some("cover.jpg".to_string()),
            extensions: Some(vec!["zip".to_string(), "rar".to_string()]),
            min_size: Some(1_024),
            max_size: Some(4_096),
            metadata_source: Some("catalog".to_string()),
        },
        actions: PluginRuleActions {
            root_folder: Some("Library".to_string()),
            move_files: vec![MoveFileRule {
                pattern: "*.jpg".to_string(),
                target: "Images".to_string(),
            }],
            move_to: Some(MoveRule {
                target_dir: "Sorted".to_string(),
                use_date: true,
                use_category: true,
            }),
            rename_pattern: Some("{title}".to_string()),
            organize_content: true,
            delete_original: true,
            use_standard_layout: false,
        },
    });
}

#[test]
fn wit_rule_conversion_preserves_every_neutral_field() {
    use wirt::bindings::wirt::plugin::rules as wit;

    let converted = wirt::conversions::convert_plugin_rule_definition(wit::PluginRuleDefinition {
        name: "Organize images".to_string(),
        category: "media".to_string(),
        description: Some("Moves image archives".to_string()),
        trigger: wit::PluginRuleTrigger {
            filename_pattern: Some("^images".to_string()),
            has_file: Some("cover.jpg".to_string()),
            extensions: Some(vec!["zip".to_string(), "rar".to_string()]),
            min_size: Some(1_024),
            max_size: Some(4_096),
            metadata_source: Some("catalog".to_string()),
        },
        actions: wit::PluginRuleActions {
            root_folder: Some("Library".to_string()),
            move_files: vec![wit::MoveFileRule {
                pattern: "*.jpg".to_string(),
                target: "Images".to_string(),
            }],
            move_to: Some(wit::MoveRule {
                target_dir: "Sorted".to_string(),
                use_date: true,
                use_category: true,
            }),
            rename_pattern: Some("{title}".to_string()),
            organize_content: true,
            delete_original: true,
            use_standard_layout: false,
        },
    });

    assert_eq!(
        converted,
        PluginRuleDefinition {
            name: "Organize images".to_string(),
            category: "media".to_string(),
            description: Some("Moves image archives".to_string()),
            trigger: PluginRuleTrigger {
                filename_pattern: Some("^images".to_string()),
                has_file: Some("cover.jpg".to_string()),
                extensions: Some(vec!["zip".to_string(), "rar".to_string()]),
                min_size: Some(1_024),
                max_size: Some(4_096),
                metadata_source: Some("catalog".to_string()),
            },
            actions: PluginRuleActions {
                root_folder: Some("Library".to_string()),
                move_files: vec![MoveFileRule {
                    pattern: "*.jpg".to_string(),
                    target: "Images".to_string(),
                }],
                move_to: Some(MoveRule {
                    target_dir: "Sorted".to_string(),
                    use_date: true,
                    use_category: true,
                }),
                rename_pattern: Some("{title}".to_string()),
                organize_content: true,
                delete_original: true,
                use_standard_layout: false,
            },
        }
    );
}
