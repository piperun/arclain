//! Layout Editor pages for customizing toolbar and info panel layouts.
//!
//! One shared `LayoutEditor<R>` (in `editor.rs`) plus shared render
//! helpers (in `render.rs`) cover both pages; the per-region
//! differences (which region to persist into, how to inject plugin
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
use crate::shared::SharedState;
use arclain_app::layout::{UiActionTypeDto, UiDisplayModeDto, UiItemDto, UiRegionDto};
use eframe::egui;

pub use editor::{
    handle_layout_editor_action, Axis, LayoutEditorAction, LayoutEditorState, Region,
};

/// Per-tab state for the Toolbar Layout editor.
pub type ToolbarLayoutState = LayoutEditorState<ToolbarRegion>;

/// Per-tab state for the Info Panel Layout editor.
pub type InfoPanelLayoutState = LayoutEditorState<InfoPanelRegion>;

// ============================================================================
// ToolbarRegion
// ============================================================================

pub struct ToolbarRegion;

impl Region for ToolbarRegion {
    const REGION: UiRegionDto = UiRegionDto::Toolbar;
    const AXIS: Axis = Axis::Horizontal;

    fn sync_plugin_items(state: &mut LayoutEditorState<Self>, shared: &SharedState) -> bool {
        use crate::features::plugins::application::{document_buttons, PluginSlot, SlotView};

        let Some(Ok(snapshot)) = shared.plugin_ui_jobs.plugin_snapshot() else {
            return false;
        };
        let enabled_plugins: Vec<_> = snapshot
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        // The editor offers the buttons the *toolbar* would draw, read out
        // of the very document the toolbar reads (see
        // `crate::core::arclain_app::toolbar_handler`). Sharing the slot
        // rather than probing is what makes that identity structural: a
        // separate read could answer differently and leave the editor
        // offering an item the toolbar has nothing to draw for.
        //
        // Unlike the info-panel region below, sharing costs nothing here.
        // A `PluginButton` slot is window-scoped and pins no archive at
        // all, so opening one from this page cannot mis-pin a plugin's
        // background writes or cache an archive-less answer for a
        // surface that wanted an archive-scoped one -- the two hazards
        // `PluginSessions::probe_extension_point` exists to avoid.
        let Some(facade) = shared.facade.as_ref() else {
            return false;
        };
        let runtime = shared.services.tokio_runtime.handle();

        let mut changed = false;

        for (plugin_id, plugin_name) in enabled_plugins {
            let slot = PluginSlot::PluginButton {
                plugin_id: plugin_id.clone(),
            };
            // Anything but `Ready` is "no answer yet" (or a failed open);
            // this sync re-runs every frame the editor is open and picks
            // the answer up then.
            let SlotView::Ready(document) = shared.plugin_sessions.view(facade, runtime, &slot)
            else {
                continue;
            };

            for button in document_buttons(&document.root) {
                let unique_id = format!("plugin_{}_{}", plugin_id, button.id);
                let action_data = format!("{}:{}", plugin_id, button.id);

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

                state.items.push(UiItemDto {
                    id: unique_id,
                    region: UiRegionDto::Toolbar,
                    group_id: Some("plugins".to_string()),
                    label: format!("{} - {}", plugin_name, button.label),
                    icon: Some("PUZZLE_PIECE".to_string()),
                    action_type: UiActionTypeDto::Plugin,
                    action_data: Some(action_data),
                    visible: true,
                    sort_order: max_sort + 10,
                    display_mode: UiDisplayModeDto::IconAndText,
                });
                changed = true;
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
    const REGION: UiRegionDto = UiRegionDto::InfoPanel;
    const AXIS: Axis = Axis::Vertical;

    fn sync_plugin_items(state: &mut LayoutEditorState<Self>, shared: &SharedState) -> bool {
        let Some(Ok(snapshot)) = shared.plugin_ui_jobs.plugin_snapshot() else {
            return false;
        };
        let enabled_plugins: Vec<_> = snapshot
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        // The editor needs a *capability* answer -- "does this plugin have
        // an info panel worth offering as a configurable item?" -- not a
        // live document. It therefore probes rather than opening the
        // archive browser's rendering slot: this page runs with no archive
        // open, and a slot opened from here would both pin the plugin's
        // background metadata writes to no archive at all and cache an
        // archive-less (empty) panel document that the browser would then
        // reuse forever. See `PluginSessions::probe_extension_point`.
        let Some(facade) = shared.facade.as_ref() else {
            return false;
        };
        let runtime = shared.services.tokio_runtime.handle();

        let mut changed = false;

        for (plugin_id, plugin_name) in enabled_plugins {
            let offers_panel = shared.plugin_sessions.probe_extension_point(
                facade,
                runtime,
                &plugin_id,
                arclain_app::plugins::PluginExtensionPointDto::Panel,
            );
            // `None` is "not answered yet"; the editor re-runs this sync on
            // later frames and picks the answer up then.
            if offers_panel != Some(true) {
                continue;
            }

            let exists = state.items.iter().any(|item| {
                item.action_type == UiActionTypeDto::Plugin
                    && item.action_data.as_ref() == Some(&plugin_id)
            });
            if exists {
                continue;
            }

            let max_sort = state.items.iter().map(|i| i.sort_order).max().unwrap_or(0);

            state.items.push(UiItemDto {
                id: format!("plugin_{}", plugin_id),
                region: UiRegionDto::InfoPanel,
                group_id: Some("plugins".to_string()),
                label: plugin_name,
                icon: Some("PUZZLE_PIECE".to_string()),
                action_type: UiActionTypeDto::Plugin,
                action_data: Some(plugin_id),
                visible: true,
                sort_order: max_sort + 10,
                display_mode: UiDisplayModeDto::default(),
            });
            changed = true;
        }

        changed
    }

    fn user_visible(item: &UiItemDto) -> bool {
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
    shared: &SharedState,
) {
    handle_layout_editor_action::<ToolbarRegion>(state, action, shared)
}

pub fn handle_info_panel_layout_action(
    state: &mut InfoPanelLayoutState,
    action: LayoutEditorAction,
    shared: &SharedState,
) {
    handle_layout_editor_action::<InfoPanelRegion>(state, action, shared)
}
