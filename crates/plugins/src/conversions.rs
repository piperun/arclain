//! WIT-binding ↔ internal-type conversions.
//!
//! The wasmtime `bindgen!` macro generates one set of types under
//! `crate::arclain::plugin::ui::*` for the WASM ABI. The host uses a
//! parallel set under `crate::types::*` that's `Send + Sync`,
//! `Serialize`-able, and otherwise idiomatic Rust. These functions
//! shuffle values across — pure data, no plugin-store / runtime
//! state involved, so they live separately from `runtime.rs`.
//!
//! Lifted out of `runtime.rs` so the runtime file can focus on
//! `WasmRuntime` / `LoadedPlugin` / `PluginInstance` instead of being
//! buried under 250 LOC of variant-by-variant matches.

use crate::types::PluginUiElement;

pub(crate) fn convert_plugin_layout(
    layout: crate::arclain::plugin::ui::PluginLayout,
) -> crate::types::PluginLayout {
    use crate::arclain::plugin::ui::PluginLayout as WitLayout;
    use crate::types::PluginLayout as InternalLayout;

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
    config: crate::arclain::plugin::ui::TopTabConfig,
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
    element: crate::arclain::plugin::ui::UiElement,
) -> PluginUiElement {
    use crate::arclain::plugin::ui::UiElement;
    use crate::types::PluginUiElement as InternalElement;

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
                        crate::arclain::plugin::ui::WarningIcon::Warning => {
                            crate::types::WarningIcon::Warning
                        }
                        crate::arclain::plugin::ui::WarningIcon::GlobeX => {
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
                crate::arclain::plugin::ui::WarningIcon::Warning => {
                    crate::types::WarningIcon::Warning
                }
                crate::arclain::plugin::ui::WarningIcon::GlobeX => {
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
    action: crate::arclain::plugin::ui::ButtonAction,
) -> crate::types::ButtonAction {
    use crate::arclain::plugin::ui::ButtonAction as WitAction;
    use crate::types::ButtonAction as InternalAction;

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
    action: crate::arclain::plugin::ui::PluginAction,
) -> crate::types::PluginAction {
    use crate::arclain::plugin::ui::PluginAction as WitAction;
    use crate::types::{PluginAction as InternalAction, ToastLevel};

    match action {
        WitAction::None => InternalAction::None,
        WitAction::EmitMetadata(json) => InternalAction::EmitMetadata { json },
        WitAction::CacheContent(req) => InternalAction::CacheContent {
            key: req.key,
            url: req.url,
        },
        WitAction::ShowToast(config) => InternalAction::ShowToast {
            message: config.message,
            level: match config.level {
                crate::arclain::plugin::ui::ToastLevel::Info => ToastLevel::Info,
                crate::arclain::plugin::ui::ToastLevel::Success => ToastLevel::Success,
                crate::arclain::plugin::ui::ToastLevel::Warning => ToastLevel::Warning,
                crate::arclain::plugin::ui::ToastLevel::Error => ToastLevel::Error,
            },
        },
        WitAction::ShowMessage(config) => InternalAction::ShowMessage {
            title: config.title,
            message: config.message,
        },
        WitAction::RefreshPanel(ep) => InternalAction::RefreshPanel {
            extension_point: ep,
        },
        WitAction::UpdateElement(update) => InternalAction::UpdateElement {
            id: update.id,
            value: update.value,
        },
        WitAction::OpenPage(page) => InternalAction::OpenPage { page },
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
