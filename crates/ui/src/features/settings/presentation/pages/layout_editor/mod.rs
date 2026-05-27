//! Layout Editor pages for customizing toolbar and info panel layouts.
//!
//! One shared `LayoutEditor<R>` (in `editor.rs`) plus shared render
//! helpers (in `render.rs`) cover both pages; the per-region
//! differences (which `UiRegion` to persist, how to inject plugin
//! items, axis direction, picker grouping, icon resolution) live in
//! the two small `Region` impls below.
//!
//! Public surface stays the same as the pre-refactor split:
//! `ToolbarLayoutState`, `InfoPanelLayoutState`,
//! `render_toolbar_layout`, `render_info_panel_layout`, plus the new
//! per-region dispatcher wrappers `handle_toolbar_layout_action` and
//! `handle_info_panel_layout_action`.

mod editor;
mod render;

use crate::shared::theme::AppTheme;
use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion, UiService};
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::{PluginExtensionPoint, PluginUiElement};
use eframe::egui;

pub use editor::{handle_layout_editor_action, Axis, LayoutEditorAction, LayoutEditorState, Region};

/// Per-tab state for the Toolbar Layout editor.
pub type ToolbarLayoutState = LayoutEditorState<ToolbarRegion>;

/// Per-tab state for the Info Panel Layout editor.
pub type InfoPanelLayoutState = LayoutEditorState<InfoPanelRegion>;

// ============================================================================
// ToolbarRegion
// ============================================================================

pub struct ToolbarRegion;

impl Region for ToolbarRegion {
    const REGION: UiRegion = UiRegion::Toolbar;
    const AXIS: Axis = Axis::Horizontal;

    fn sync_plugin_items(
        state: &mut LayoutEditorState<Self>,
        manager: &PluginManager,
    ) -> bool {
        let enabled_plugins: Vec<_> = manager
            .list_plugins()
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
            .collect();

        let mut changed = false;

        for (plugin_id, plugin_name) in enabled_plugins {
            // try_lock: if a worker is mid-fetch we skip this plugin
            // and pick it up next frame.
            let elements = match manager.try_with_plugin_instance(&plugin_id, |instance| {
                instance
                    .get_ui_layout(PluginExtensionPoint::PluginButton)
                    .unwrap_or_default()
            }) {
                Some(Some(layout)) => layout,
                _ => Default::default(),
            };

            if elements.is_empty() {
                continue;
            }

            for element in elements.flatten() {
                if let PluginUiElement::Button {
                    id: btn_id, label, ..
                } = element
                {
                    let unique_id = format!("plugin_{}_{}", plugin_id, btn_id);
                    let action_data = format!("{}:{}", plugin_id, btn_id);

                    let exists = state.items.iter().any(|item| item.id == unique_id);
                    if exists {
                        continue;
                    }

                    // Migrate from the legacy ID format (`plugin_{id}`,
                    // one entry per plugin) to the per-button format
                    // (`plugin_{id}_{btn_id}`). Remove the legacy entry
                    // if it's still hanging around.
                    let legacy_id = format!("plugin_{}", plugin_id);
                    if let Some(pos) = state.items.iter().position(|i| i.id == legacy_id) {
                        state.items.remove(pos);
                    }

                    let max_sort = state.items.iter().map(|i| i.sort_order).max().unwrap_or(0);

                    state.items.push(UiItem {
                        id: unique_id,
                        region: UiRegion::Toolbar,
                        group_id: Some("plugins".to_string()),
                        label: format!("{} - {}", plugin_name, label),
                        icon: Some("PUZZLE_PIECE".to_string()),
                        action_type: ActionType::Plugin,
                        action_data: Some(action_data),
                        visible: true,
                        sort_order: max_sort + 10,
                        display_mode: DisplayMode::IconAndText,
                    });
                    changed = true;
                }
            }
        }

        changed
    }

    fn picker_groups() -> &'static [(&'static str, &'static str)] {
        &[
            ("navigation", "Navigation"),
            ("file_actions", "File Actions"),
            ("view", "View Mode"),
            ("panels", "Panel Toggles"),
            ("plugins", "Plugins"),
        ]
    }

    fn icon_for_name(name: &str) -> Option<&'static str> {
        Some(match name {
            "ARROW_LEFT" => egui_phosphor::regular::ARROW_LEFT,
            "ARROW_RIGHT" => egui_phosphor::regular::ARROW_RIGHT,
            "ARROW_UP" => egui_phosphor::regular::ARROW_UP,
            "FOLDER_OPEN" => egui_phosphor::regular::FOLDER_OPEN,
            "EXPORT" => egui_phosphor::regular::EXPORT,
            "PLUS" => egui_phosphor::regular::PLUS,
            "TRASH" => egui_phosphor::regular::TRASH,
            "PACKAGE" => egui_phosphor::regular::PACKAGE,
            "FOLDERS" => egui_phosphor::regular::FOLDERS,
            "LIST" => egui_phosphor::regular::LIST,
            "GRID_FOUR" => egui_phosphor::regular::GRID_FOUR,
            "LOCK" => egui_phosphor::regular::LOCK,
            "TREE_STRUCTURE" => egui_phosphor::regular::TREE_STRUCTURE,
            "INFO" => egui_phosphor::regular::INFO,
            _ => egui_phosphor::regular::QUESTION,
        })
    }
}

// ============================================================================
// InfoPanelRegion
// ============================================================================

pub struct InfoPanelRegion;

/// Items the user shouldn't see in the info-panel picker. Currently
/// just the host-managed `info.plugin_metadata` block, which appears
/// in the live panel but is driven by plugin metadata rather than
/// user configuration.
const INFO_PANEL_INTERNAL_IDS: &[&str] = &["info.plugin_metadata"];

impl Region for InfoPanelRegion {
    const REGION: UiRegion = UiRegion::InfoPanel;
    const AXIS: Axis = Axis::Vertical;

    fn sync_plugin_items(
        state: &mut LayoutEditorState<Self>,
        manager: &PluginManager,
    ) -> bool {
        let enabled_plugins: Vec<_> = manager
            .list_plugins()
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
            .collect();

        let mut changed = false;

        for (plugin_id, plugin_name) in enabled_plugins {
            // Probe whether the plugin contributes a panel section. As
            // with the toolbar sync, we try_lock and skip on contention.
            let has_info_panel = match manager.try_with_plugin_instance(&plugin_id, |instance| {
                if let Ok(elements) = instance.get_ui_layout(PluginExtensionPoint::Panel) {
                    !elements.is_empty()
                } else {
                    false
                }
            }) {
                Some(Some(v)) => v,
                _ => false,
            };

            if !has_info_panel {
                continue;
            }

            let exists = state.items.iter().any(|item| {
                item.action_type == ActionType::Plugin
                    && item.action_data.as_ref() == Some(&plugin_id)
            });
            if exists {
                continue;
            }

            let max_sort = state.items.iter().map(|i| i.sort_order).max().unwrap_or(0);

            state.items.push(UiItem {
                id: format!("plugin_{}", plugin_id),
                region: UiRegion::InfoPanel,
                group_id: Some("plugins".to_string()),
                label: plugin_name,
                icon: Some("PUZZLE_PIECE".to_string()),
                action_type: ActionType::Plugin,
                action_data: Some(plugin_id),
                visible: true,
                sort_order: max_sort + 10,
                display_mode: DisplayMode::default(),
            });
            changed = true;
        }

        changed
    }

    fn user_visible(item: &UiItem) -> bool {
        !INFO_PANEL_INTERNAL_IDS.contains(&item.id.as_str())
    }
}

// ============================================================================
// Public entry points
// ============================================================================

pub fn render_toolbar_layout(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ToolbarLayoutState,
) -> Option<LayoutEditorAction> {
    render::render_layout_editor::<ToolbarRegion>(ui, theme, state)
}

pub fn render_info_panel_layout(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut InfoPanelLayoutState,
) -> Option<LayoutEditorAction> {
    render::render_layout_editor::<InfoPanelRegion>(ui, theme, state)
}

pub fn handle_toolbar_layout_action(
    state: &mut ToolbarLayoutState,
    action: LayoutEditorAction,
    ui_service: Option<&UiService>,
    plugin_manager: Option<&PluginManager>,
) {
    handle_layout_editor_action::<ToolbarRegion>(state, action, ui_service, plugin_manager)
}

pub fn handle_info_panel_layout_action(
    state: &mut InfoPanelLayoutState,
    action: LayoutEditorAction,
    ui_service: Option<&UiService>,
    plugin_manager: Option<&PluginManager>,
) {
    handle_layout_editor_action::<InfoPanelRegion>(state, action, ui_service, plugin_manager)
}
