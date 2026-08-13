//! Pure conversions from generated WIT values into Wirt's neutral model.

use crate::{
    BadgeConfig, BadgeLevel, ButtonAction, KeyValuePair, PluginAction, PluginLayout,
    PluginUiElement, SidebarWidth, SizeHint, SpacingStep, TextRole, ToastLevel, ToolbarButton,
    TopTabConfig, WarningIcon,
};
use crate::{MoveFileRule, MoveRule, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger};

pub fn convert_plugin_layout(
    layout: crate::bindings::wirt::plugin::ui::PluginLayout,
) -> PluginLayout {
    use crate::bindings::wirt::plugin::ui::PluginLayout as WitLayout;

    match layout {
        WitLayout::Single(elements) => PluginLayout::Single {
            elements: elements.into_iter().map(convert_ui_element).collect(),
        },
        WitLayout::Split(config) => PluginLayout::Split {
            sidebar: config.sidebar.into_iter().map(convert_ui_element).collect(),
            content: config.content.into_iter().map(convert_ui_element).collect(),
            width: config.width.map(convert_sidebar_width),
        },
    }
}

pub fn convert_top_tab_config(
    config: crate::bindings::wirt::plugin::ui::TopTabConfig,
) -> TopTabConfig {
    TopTabConfig {
        id: config.id,
        label: config.label,
        icon: config.icon,
        badge: config.badge.map(|badge| BadgeConfig {
            count: badge.count,
            dot: badge.dot,
            level: convert_badge_level(badge.level),
        }),
        priority: config.priority,
    }
}

pub fn convert_ui_element(
    element: crate::bindings::wirt::plugin::ui::UiElement,
) -> PluginUiElement {
    use crate::bindings::wirt::plugin::ui::{UiElement, WarningIcon as WitWarningIcon};

    match element {
        UiElement::Label(config) => PluginUiElement::Label {
            text: config.text,
            role: convert_text_role(config.role),
        },
        UiElement::SectionHeader(config) => PluginUiElement::SectionHeader {
            title: config.title,
            level: config.level,
            description: config.description,
        },
        UiElement::Button(config) => PluginUiElement::Button {
            id: config.id,
            label: config.label,
            action: config.action.map(convert_button_action),
        },
        UiElement::TextInput(config) => PluginUiElement::TextInput {
            id: config.id,
            label: config.label,
            value: config.value,
            placeholder: config.placeholder,
        },
        UiElement::Checkbox(config) => PluginUiElement::Checkbox {
            id: config.id,
            label: config.label,
            checked: config.checked,
        },
        UiElement::RadioGroup(config) => PluginUiElement::RadioGroup {
            id: config.id,
            label: config.label,
            options: config.options,
            selected: config.selected,
        },
        UiElement::Slider(config) => PluginUiElement::Slider {
            id: config.id,
            label: config.label,
            value: config.value,
            min: config.min,
            max: config.max,
            step: config.step,
        },
        UiElement::Dropdown(config) => PluginUiElement::Dropdown {
            id: config.id,
            label: config.label,
            options: config.options,
            selected: config.selected,
        },
        UiElement::Separator => PluginUiElement::Separator,
        UiElement::Space(step) => PluginUiElement::Space {
            step: convert_spacing_step(step),
        },
        UiElement::Image(config) => PluginUiElement::Image {
            cache_key: config.cache_key,
            url: config.url,
            height: config.height.map(convert_size_hint),
        },
        UiElement::Tabs(config) => PluginUiElement::Tabs {
            id: config.id,
            tabs: config.tabs,
            selected: config.selected,
        },
        UiElement::ListContainer(config) => PluginUiElement::ListContainer {
            id: config.id,
            items: config
                .items
                .into_iter()
                .map(|item| PluginUiElement::ListItem {
                    id: item.id,
                    title: item.title,
                    subtitle: item.subtitle,
                    badge: item.badge,
                    image_key: item.image_key,
                    image_url: item.image_url,
                    selected: item.selected,
                    warning_icon: item.warning_icon.map(|icon| match icon {
                        WitWarningIcon::Warning => WarningIcon::Warning,
                        WitWarningIcon::GlobeX => WarningIcon::GlobeX,
                    }),
                })
                .collect(),
            height: config.height.map(convert_size_hint),
            empty_message: config.empty_message,
        },
        UiElement::Loading(config) => PluginUiElement::Loading {
            message: config.message,
        },
        UiElement::Warning(config) => PluginUiElement::Warning {
            icon: match config.icon {
                WitWarningIcon::Warning => WarningIcon::Warning,
                WitWarningIcon::GlobeX => WarningIcon::GlobeX,
            },
            message: config.message,
        },
        UiElement::TagChips(config) => PluginUiElement::TagChips {
            tags: config.tags,
            max_display: config.max_display,
        },
        UiElement::Toolbar(config) => PluginUiElement::Toolbar {
            buttons: config
                .buttons
                .into_iter()
                .map(|button| ToolbarButton {
                    id: button.id,
                    label: button.label,
                    icon: button.icon,
                    primary: button.primary,
                    spacer_before: button.spacer_before,
                })
                .collect(),
        },
        UiElement::Carousel(config) => PluginUiElement::Carousel {
            id: config.id,
            images: config.images,
            current_index: config.current_index as usize,
            height: config.height.map(convert_size_hint),
            enable_lightbox: config.enable_lightbox,
        },
        UiElement::KeyValueList(config) => PluginUiElement::KeyValueList {
            items: config
                .items
                .into_iter()
                .map(|pair| KeyValuePair {
                    key: pair.key,
                    value: pair.value,
                })
                .collect(),
            columns: config.columns,
        },
        UiElement::MetadataGrid(config) => PluginUiElement::MetadataGrid {
            items: config
                .items
                .into_iter()
                .map(|pair| KeyValuePair {
                    key: pair.key,
                    value: pair.value,
                })
                .collect(),
            columns: config.columns,
        },
        UiElement::GroupBegin(header) => PluginUiElement::GroupBegin {
            title: header.title,
            description: header.description,
        },
        UiElement::GroupEnd => PluginUiElement::GroupEnd,
    }
}

fn convert_text_role(role: crate::bindings::wirt::plugin::ui::TextRole) -> TextRole {
    use crate::bindings::wirt::plugin::ui::TextRole as WitRole;

    match role {
        WitRole::Title => TextRole::Title,
        WitRole::Subtitle => TextRole::Subtitle,
        WitRole::Body => TextRole::Body,
        WitRole::Caption => TextRole::Caption,
        WitRole::Emphasis => TextRole::Emphasis,
    }
}

fn convert_size_hint(hint: crate::bindings::wirt::plugin::ui::SizeHint) -> SizeHint {
    use crate::bindings::wirt::plugin::ui::SizeHint as WitHint;

    match hint {
        WitHint::Compact => SizeHint::Compact,
        WitHint::Regular => SizeHint::Regular,
        WitHint::Tall => SizeHint::Tall,
    }
}

fn convert_sidebar_width(width: crate::bindings::wirt::plugin::ui::SidebarWidth) -> SidebarWidth {
    use crate::bindings::wirt::plugin::ui::SidebarWidth as WitWidth;

    match width {
        WitWidth::Narrow => SidebarWidth::Narrow,
        WitWidth::Medium => SidebarWidth::Medium,
        WitWidth::Wide => SidebarWidth::Wide,
    }
}

fn convert_badge_level(level: crate::bindings::wirt::plugin::ui::BadgeLevel) -> BadgeLevel {
    use crate::bindings::wirt::plugin::ui::BadgeLevel as WitLevel;

    match level {
        WitLevel::Neutral => BadgeLevel::Neutral,
        WitLevel::Info => BadgeLevel::Info,
        WitLevel::Success => BadgeLevel::Success,
        WitLevel::Warning => BadgeLevel::Warning,
        WitLevel::Error => BadgeLevel::Error,
    }
}

fn convert_spacing_step(step: crate::bindings::wirt::plugin::ui::SpacingStep) -> SpacingStep {
    use crate::bindings::wirt::plugin::ui::SpacingStep as WitStep;

    match step {
        WitStep::Small => SpacingStep::Small,
        WitStep::Medium => SpacingStep::Medium,
        WitStep::Large => SpacingStep::Large,
    }
}

pub fn convert_button_action(
    action: crate::bindings::wirt::plugin::ui::ButtonAction,
) -> ButtonAction {
    use crate::bindings::wirt::plugin::ui::ButtonAction as WitAction;

    match action {
        WitAction::None => ButtonAction::None,
        WitAction::ShowDialog(id) => ButtonAction::ShowDialog { id },
        WitAction::CloseDialog => ButtonAction::CloseDialog,
        WitAction::OpenPage(id) => ButtonAction::OpenPage { id },
        WitAction::ClosePage => ButtonAction::ClosePage,
        WitAction::Custom(value) => ButtonAction::Custom(value),
    }
}

pub fn convert_plugin_action(
    action: crate::bindings::wirt::plugin::ui::PluginAction,
) -> PluginAction {
    use crate::bindings::wirt::plugin::ui::{PluginAction as WitAction, ToastLevel as WitLevel};

    match action {
        WitAction::None => PluginAction::None,
        WitAction::ShowToast(config) => PluginAction::ShowToast {
            message: config.message,
            level: match config.level {
                WitLevel::Info => ToastLevel::Info,
                WitLevel::Success => ToastLevel::Success,
                WitLevel::Warning => ToastLevel::Warning,
                WitLevel::Error => ToastLevel::Error,
            },
        },
        WitAction::RefreshPanel(extension_point) => PluginAction::RefreshPanel { extension_point },
        WitAction::CloseDialog => PluginAction::CloseDialog,
        WitAction::CopyToClipboard(text) => PluginAction::CopyToClipboard { text },
        WitAction::OpenLightbox(config) => PluginAction::OpenLightbox {
            images: config.images,
            start_index: config.start_index as usize,
            title: config.title,
        },
        WitAction::SetPageDisplayName(name) => PluginAction::SetPageDisplayName { name },
        WitAction::RequestFetch(key) => PluginAction::RequestFetch { key },
    }
}

pub fn convert_plugin_rule_definition(
    definition: crate::bindings::wirt::plugin::rules::PluginRuleDefinition,
) -> PluginRuleDefinition {
    PluginRuleDefinition {
        name: definition.name,
        category: definition.category,
        description: definition.description,
        trigger: convert_plugin_rule_trigger(definition.trigger),
        actions: convert_plugin_rule_actions(definition.actions),
    }
}

fn convert_plugin_rule_trigger(
    trigger: crate::bindings::wirt::plugin::rules::PluginRuleTrigger,
) -> PluginRuleTrigger {
    PluginRuleTrigger {
        filename_pattern: trigger.filename_pattern,
        has_file: trigger.has_file,
        extensions: trigger.extensions,
        min_size: trigger.min_size,
        max_size: trigger.max_size,
        metadata_source: trigger.metadata_source,
    }
}

fn convert_plugin_rule_actions(
    actions: crate::bindings::wirt::plugin::rules::PluginRuleActions,
) -> PluginRuleActions {
    PluginRuleActions {
        root_folder: actions.root_folder,
        move_files: actions
            .move_files
            .into_iter()
            .map(|rule| MoveFileRule {
                pattern: rule.pattern,
                target: rule.target,
            })
            .collect(),
        move_to: actions.move_to.map(|rule| MoveRule {
            target_dir: rule.target_dir,
            use_date: rule.use_date,
            use_category: rule.use_category,
        }),
        rename_pattern: actions.rename_pattern,
        organize_content: actions.organize_content,
        delete_original: actions.delete_original,
        use_standard_layout: actions.use_standard_layout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::wirt::plugin::ui::{
        BadgeConfig as WitBadgeConfig, BadgeLevel as WitBadgeLevel, TopTabConfig as WitTopTabConfig,
    };
    use crate::BadgeLevel;

    /// A tab's badge crosses the boundary outside the UI element tree
    /// entirely -- it is chrome, reached from `get-top-tabs` rather than
    /// from a layout -- so this is the one styling vocabulary neither
    /// `convert_ui_element` nor `convert_plugin_layout` ever sees. Every
    /// field is destructured with no `..` so that a field surviving here
    /// that should no longer exist fails to compile.
    #[test]
    fn badge_level_survives_the_conversion() {
        for (wit, expected) in [
            (WitBadgeLevel::Neutral, BadgeLevel::Neutral),
            (WitBadgeLevel::Info, BadgeLevel::Info),
            (WitBadgeLevel::Success, BadgeLevel::Success),
            (WitBadgeLevel::Warning, BadgeLevel::Warning),
            (WitBadgeLevel::Error, BadgeLevel::Error),
        ] {
            let config = convert_top_tab_config(WitTopTabConfig {
                id: "t".to_string(),
                label: "T".to_string(),
                icon: "*".to_string(),
                badge: Some(WitBadgeConfig {
                    count: Some(3),
                    dot: false,
                    level: wit,
                }),
                priority: 5,
            });
            let BadgeConfig { count, dot, level } = config.badge.expect("the badge survives");
            assert_eq!(count, Some(3));
            assert!(!dot);
            assert_eq!(level, expected);
        }
    }

    /// A tab without a badge is the common case -- three of the four
    /// bundled guests register no badge at all -- and it must stay absent
    /// rather than acquiring a level nobody asked for.
    #[test]
    fn a_tab_without_a_badge_converts_without_one() {
        let config = convert_top_tab_config(WitTopTabConfig {
            id: "t".to_string(),
            label: "T".to_string(),
            icon: "*".to_string(),
            badge: None,
            priority: 5,
        });
        assert_eq!(config.badge, None);
    }
}
