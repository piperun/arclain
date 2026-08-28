//! Arclain-specific adapters from Wirt's neutral model.

pub(crate) fn convert_plugin_rule_definition(
    definition: wirt::rules::PluginRuleDefinition,
) -> arclain_core::OrganizationRule {
    arclain_core::OrganizationRule {
        name: definition.name,
        priority: 100,
        is_enabled: true,
        trigger: convert_plugin_rule_trigger(definition.trigger),
        actions: convert_plugin_rule_actions(definition.actions),
        ..Default::default()
    }
}

fn convert_plugin_rule_trigger(
    trigger: wirt::rules::PluginRuleTrigger,
) -> arclain_core::RuleTrigger {
    arclain_core::RuleTrigger {
        filename_pattern: trigger.filename_pattern,
        has_file: trigger.has_file,
        metadata_source: trigger.metadata_source,
    }
}

/// Wirt's rule vocabulary still speaks in a root folder, a move list and
/// a standard-layout boolean, because that is the plugin ABI and
/// changing it is a change to every plugin already built. The same
/// translation a rule saved under that vocabulary goes through on read
/// converts it here, so a plugin-supplied rule and a stored one produce
/// the same layout rather than two shapes that drift apart.
fn convert_plugin_rule_actions(
    actions: wirt::rules::PluginRuleActions,
) -> arclain_core::RuleActions {
    let move_files = actions
        .move_files
        .into_iter()
        .map(|rule| arclain_core::MoveAction {
            pattern: rule.pattern,
            target: rule.target,
        })
        .collect();

    arclain_core::RuleActions {
        output_name: None,
        layout: arclain_core::features::organization::layout_from_legacy_actions(
            actions.root_folder,
            move_files,
            actions.use_standard_layout,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wirt::rules::{
        MoveFileRule, MoveRule, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger,
    };

    #[test]
    fn arclain_rule_adapter_keeps_supported_fields_and_existing_defaults() {
        let converted = convert_plugin_rule_definition(PluginRuleDefinition {
            name: "Rule".to_string(),
            category: "neutral-only".to_string(),
            description: Some("neutral-only".to_string()),
            trigger: PluginRuleTrigger {
                filename_pattern: Some("*.zip".to_string()),
                has_file: Some("cover.jpg".to_string()),
                extensions: Some(vec!["zip".to_string()]),
                min_size: Some(10),
                max_size: Some(20),
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

        assert_eq!(converted.id, 0);
        assert_eq!(converted.name, "Rule");
        assert_eq!(converted.priority, 100);
        assert!(converted.is_enabled);
        assert_eq!(converted.trigger.filename_pattern.as_deref(), Some("*.zip"));
        assert_eq!(converted.trigger.has_file.as_deref(), Some("cover.jpg"));
        assert_eq!(
            converted.trigger.metadata_source.as_deref(),
            Some("catalog")
        );
        assert_eq!(converted.actions.output_name, None);
        assert_eq!(converted.actions.layout.name, "Library");
        assert_eq!(
            converted.actions.layout.place,
            vec![arclain_core::features::organization::layout::Placement {
                from: arclain_core::features::organization::layout::Source::Matching(
                    "*.jpg".to_string()
                ),
                into: "Images".to_string(),
            }],
            "a plugin's move list becomes the layout's placements, in order"
        );
    }
}
