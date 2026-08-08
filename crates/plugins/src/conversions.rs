//! WIT-binding ↔ host-type conversions.
//!
//! The wasmtime `bindgen!` macro generates one set of types under
//! `wirt::bindings::wirt::plugin::*` for the WASM ABI. The host uses a
//! parallel set under `crate::types::*` (UI elements, plugin actions,
//! tab configs) and `arclain_core::*` (organization rules) that's
//! `Send + Sync`, `Serialize`-able, and otherwise idiomatic Rust.
//! These functions / `From` impls shuffle values across — pure data,
//! no plugin-store / runtime state involved.
//!
//! Lifted out of `runtime.rs` (UI-element conversions) and `types.rs`
//! (rule `From` impls — they're not type definitions and don't belong
//! next to the host-side `PluginUiElement` / `PluginManifest` shapes).

use crate::types::PluginUiElement;
use wirt::bindings::wirt::plugin::rules as wit_rules;

pub(crate) fn convert_plugin_layout(
    layout: wirt::bindings::wirt::plugin::ui::PluginLayout,
) -> crate::types::PluginLayout {
    use crate::types::PluginLayout as InternalLayout;
    use wirt::bindings::wirt::plugin::ui::PluginLayout as WitLayout;

    match layout {
        WitLayout::Single(elements) => InternalLayout::Single {
            elements: elements.into_iter().map(convert_ui_element).collect(),
        },
        WitLayout::Split(config) => InternalLayout::Split {
            sidebar: config.sidebar.into_iter().map(convert_ui_element).collect(),
            content: config.content.into_iter().map(convert_ui_element).collect(),
            sidebar_width: config.sidebar_width,
        },
    }
}

pub(crate) fn convert_top_tab_config(
    config: wirt::bindings::wirt::plugin::ui::TopTabConfig,
) -> crate::types::TopTabConfig {
    crate::types::TopTabConfig {
        id: config.id,
        label: config.label,
        icon: config.icon,
        badge: config.badge.map(|b| crate::types::BadgeConfig {
            count: b.count,
            dot: b.dot,
            color: b.color,
        }),
        priority: config.priority,
    }
}

pub(crate) fn convert_ui_element(
    element: wirt::bindings::wirt::plugin::ui::UiElement,
) -> PluginUiElement {
    use crate::types::PluginUiElement as InternalElement;
    use wirt::bindings::wirt::plugin::ui::UiElement;

    match element {
        UiElement::Label(config) => InternalElement::Label {
            text: config.text,
            bold: config.bold,
            size: config.size,
        },
        UiElement::SectionHeader(config) => InternalElement::SectionHeader {
            title: config.title,
            level: config.level,
            description: config.description,
        },
        UiElement::Button(config) => {
            let action = config.action.map(convert_button_action);
            InternalElement::Button {
                id: config.id,
                label: config.label,
                action,
            }
        }
        UiElement::TextInput(config) => InternalElement::TextInput {
            id: config.id,
            label: config.label,
            value: config.value,
            placeholder: config.placeholder,
        },
        UiElement::Checkbox(config) => InternalElement::Checkbox {
            id: config.id,
            label: config.label,
            checked: config.checked,
        },
        UiElement::RadioGroup(config) => InternalElement::RadioGroup {
            id: config.id,
            label: config.label,
            options: config.options,
            selected: config.selected,
        },
        UiElement::Slider(config) => InternalElement::Slider {
            id: config.id,
            label: config.label,
            value: config.value,
            min: config.min,
            max: config.max,
            step: config.step,
        },
        UiElement::Dropdown(config) => InternalElement::Dropdown {
            id: config.id,
            label: config.label,
            options: config.options,
            selected: config.selected,
        },
        UiElement::Separator => InternalElement::Separator,
        UiElement::Space(size) => InternalElement::Space { size },
        UiElement::Image(config) => InternalElement::Image {
            cache_key: config.cache_key,
            url: config.url,
            max_height: config.max_height,
        },
        UiElement::Tabs(config) => InternalElement::Tabs {
            id: config.id,
            tabs: config.tabs,
            selected: config.selected,
        },
        UiElement::ListContainer(config) => InternalElement::ListContainer {
            id: config.id,
            items: config
                .items
                .into_iter()
                .map(|item| InternalElement::ListItem {
                    id: item.id,
                    title: item.title,
                    subtitle: item.subtitle,
                    badge: item.badge,
                    image_key: item.image_key,
                    image_url: item.image_url,
                    selected: item.selected,
                    warning_icon: item.warning_icon.map(|i| match i {
                        wirt::bindings::wirt::plugin::ui::WarningIcon::Warning => {
                            crate::types::WarningIcon::Warning
                        }
                        wirt::bindings::wirt::plugin::ui::WarningIcon::GlobeX => {
                            crate::types::WarningIcon::GlobeX
                        }
                    }),
                })
                .collect(),
            max_height: config.max_height,
            empty_message: config.empty_message,
        },
        UiElement::Loading(config) => InternalElement::Loading {
            message: config.message,
        },
        UiElement::Warning(config) => InternalElement::Warning {
            icon: match config.icon {
                wirt::bindings::wirt::plugin::ui::WarningIcon::Warning => {
                    crate::types::WarningIcon::Warning
                }
                wirt::bindings::wirt::plugin::ui::WarningIcon::GlobeX => {
                    crate::types::WarningIcon::GlobeX
                }
            },
            message: config.message,
        },
        UiElement::TagChips(config) => InternalElement::TagChips {
            tags: config.tags,
            max_display: config.max_display,
        },
        UiElement::Toolbar(config) => InternalElement::Toolbar {
            buttons: config
                .buttons
                .into_iter()
                .map(|b| crate::types::ToolbarButton {
                    id: b.id,
                    label: b.label,
                    icon: b.icon,
                    primary: b.primary,
                    spacer_before: b.spacer_before,
                })
                .collect(),
        },
        UiElement::Carousel(config) => InternalElement::Carousel {
            id: config.id,
            images: config.images,
            current_index: config.current_index as usize,
            max_height: config.max_height,
            thumbnail_height: config.thumbnail_height,
            enable_lightbox: config.enable_lightbox,
        },
        UiElement::KeyValueList(config) => InternalElement::KeyValueList {
            items: config
                .items
                .into_iter()
                .map(|kv| crate::types::KeyValuePair {
                    key: kv.key,
                    value: kv.value,
                })
                .collect(),
            columns: config.columns,
        },
        UiElement::MetadataGrid(config) => InternalElement::MetadataGrid {
            items: config
                .items
                .into_iter()
                .map(|kv| crate::types::KeyValuePair {
                    key: kv.key,
                    value: kv.value,
                })
                .collect(),
            columns: config.columns,
        },
        UiElement::GroupBegin(header) => InternalElement::GroupBegin {
            title: header.title,
            description: header.description,
        },
        UiElement::GroupEnd => InternalElement::GroupEnd,
    }
}

pub(crate) fn convert_button_action(
    action: wirt::bindings::wirt::plugin::ui::ButtonAction,
) -> crate::types::ButtonAction {
    use crate::types::ButtonAction as InternalAction;
    use wirt::bindings::wirt::plugin::ui::ButtonAction as WitAction;

    match action {
        WitAction::None => InternalAction::None,
        WitAction::ShowDialog(id) => InternalAction::ShowDialog { id },
        WitAction::CloseDialog => InternalAction::CloseDialog,
        WitAction::OpenPage(id) => InternalAction::OpenPage { id },
        WitAction::ClosePage => InternalAction::ClosePage,
        WitAction::Custom(s) => InternalAction::Custom(s),
    }
}

pub(crate) fn convert_plugin_action(
    action: wirt::bindings::wirt::plugin::ui::PluginAction,
) -> crate::types::PluginAction {
    use crate::types::{PluginAction as InternalAction, ToastLevel};
    use wirt::bindings::wirt::plugin::ui::PluginAction as WitAction;

    match action {
        WitAction::None => InternalAction::None,
        WitAction::ShowToast(config) => InternalAction::ShowToast {
            message: config.message,
            level: match config.level {
                wirt::bindings::wirt::plugin::ui::ToastLevel::Info => ToastLevel::Info,
                wirt::bindings::wirt::plugin::ui::ToastLevel::Success => ToastLevel::Success,
                wirt::bindings::wirt::plugin::ui::ToastLevel::Warning => ToastLevel::Warning,
                wirt::bindings::wirt::plugin::ui::ToastLevel::Error => ToastLevel::Error,
            },
        },
        WitAction::RefreshPanel(ep) => InternalAction::RefreshPanel {
            extension_point: ep,
        },
        WitAction::CloseDialog => InternalAction::CloseDialog,
        WitAction::CopyToClipboard(text) => InternalAction::CopyToClipboard { text },
        WitAction::OpenLightbox(config) => InternalAction::OpenLightbox {
            images: config.images,
            start_index: config.start_index as usize,
            title: config.title,
        },
        WitAction::SetPageDisplayName(name) => InternalAction::SetPageDisplayName { name },
        WitAction::RequestFetch(key) => InternalAction::RequestFetch { key },
    }
}

// ============================================================================
// Organization rules: WIT → arclain_core
// ============================================================================
//
// This conversion folds a plugin's `get_default_rules` return into the host's
// organization rule store (`Vec<wit_rules::PluginRuleDefinition>` →
// `Vec<arclain_core::OrganizationRule>`). It cannot use `From` because both
// types are now defined outside this host crate.

pub(crate) fn convert_plugin_rule_definition(
    def: wit_rules::PluginRuleDefinition,
) -> arclain_core::OrganizationRule {
    arclain_core::OrganizationRule {
        name: def.name,
        priority: 100, // Plugins get high priority by default? Or config?
        is_enabled: true,
        trigger: convert_plugin_rule_trigger(def.trigger),
        actions: convert_plugin_rule_actions(def.actions),
        ..Default::default()
    }
}

fn convert_plugin_rule_trigger(t: wit_rules::PluginRuleTrigger) -> arclain_core::RuleTrigger {
    arclain_core::RuleTrigger {
        filename_pattern: t.filename_pattern,
        has_file: t.has_file,
        metadata_source: t.metadata_source,
    }
}

fn convert_plugin_rule_actions(a: wit_rules::PluginRuleActions) -> arclain_core::RuleActions {
    arclain_core::RuleActions {
        root_folder: a.root_folder,
        move_files: a
            .move_files
            .into_iter()
            .map(convert_move_file_rule)
            .collect(),
        use_standard_layout: a.use_standard_layout,
        ..Default::default()
    }
}

fn convert_move_file_rule(m: wit_rules::MoveFileRule) -> arclain_core::MoveAction {
    arclain_core::MoveAction {
        pattern: m.pattern,
        target: m.target,
    }
}
