//! Toolbar Layout Editor
//!
//! Visual editor for customizing toolbar button layout with:
//! - Live toolbar preview with click-to-select
//! - Selection area with move left/right arrows
//! - Item picker grid for toggling visibility

use crate::shared::theme::AppTheme;
use arclain_core::UiService;
use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion};
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::{PluginExtensionPoint, PluginUiElement};
use arclain_theme::spacing;
use arclain_widgets::SelectableChip;
use eframe::egui;

/// State for toolbar layout editor
pub struct ToolbarLayoutState {
    pub items: Vec<UiItem>,
    pub dirty: bool,
    pub loaded: bool,
    /// Currently selected item ID (for reordering)
    pub selected_item_id: Option<String>,
}

impl Default for ToolbarLayoutState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            dirty: false,
            loaded: false,
            selected_item_id: None,
        }
    }
}

impl ToolbarLayoutState {
    /// Load toolbar items from database via UiService
    pub fn load_from_service(&mut self, service: &UiService) {
        if self.loaded {
            return;
        }
        if let Ok(items) = service.list_toolbar_items() {
            self.items = items;
        }
        self.loaded = true;
        self.dirty = false;
    }

    /// Save toolbar items to database via UiService
    pub fn save_to_service(&mut self, service: &UiService) {
        if !self.dirty {
            return;
        }
        let _ = service.upsert_items(&self.items);
        self.dirty = false;
    }
}

/// Render the Toolbar Layout Editor page
pub fn render_toolbar_layout(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    ui_service: Option<&UiService>,
    state: &mut ToolbarLayoutState,
    plugin_manager: Option<&PluginManager>,
) {
    // Load items from database via UiService
    if !state.loaded {
        if let Some(service) = ui_service {
            state.load_from_service(service);
        }
    }

    // Sync plugin items into the list
    if let Some(manager) = plugin_manager {
        sync_plugin_items(state, manager);
    }

    // The header with Save/Reset is now rendered by SettingsHeader in ui.rs
    // This page only renders the content area

    // Section: Live Preview (click to select)
    ui.label(
        egui::RichText::new("Toolbar Preview (click item to select)")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_clickable_preview(ui, theme, &mut state.items, &mut state.selected_item_id);

    ui.add_space(16.0);

    // Selection area (only shown when something is selected)
    if state.selected_item_id.is_some() {
        render_selection_area(
            ui,
            theme,
            &mut state.items,
            &mut state.selected_item_id,
            &mut state.dirty,
        );
        ui.add_space(16.0);
    }

    // Section: Available Items (click to toggle visibility)
    ui.label(
        egui::RichText::new("Available Items (click to show/hide)")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_item_picker(
        ui,
        theme,
        &mut state.items,
        &mut state.selected_item_id,
        &mut state.dirty,
    );
}

/// Sync plugins into the items list
fn sync_plugin_items(state: &mut ToolbarLayoutState, manager: &PluginManager) {
    let enabled_plugins: Vec<_> = manager
        .list_plugins()
        .iter()
        .filter(|p| p.enabled)
        .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
        .collect();

    let mut changed = false;

    for (plugin_id, plugin_name) in enabled_plugins {
        // Get toolbar elements via try_lock; if a worker is mid-fetch
        // we skip this plugin in the current render of the layout
        // editor and pick it up next frame.
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

        // Create item for each button
        for element in elements.flatten() {
            if let PluginUiElement::Button {
                id: btn_id, label, ..
            } = element
            {
                let unique_id = format!("plugin_{}_{}", plugin_id, btn_id);
                // Action data format: "plugin_id:button_id"
                let action_data = format!("{}:{}", plugin_id, btn_id);

                // Check if item exists (by ID or legacy check)
                let exists = state.items.iter().any(|item| item.id == unique_id);

                if !exists {
                    // Check if there is a legacy item (whole plugin) to remove
                    // Legacy ID format was "plugin_{plugin_id}"
                    let legacy_id = format!("plugin_{}", plugin_id);
                    if let Some(pos) = state.items.iter().position(|i| i.id == legacy_id) {
                        state.items.remove(pos);
                    }

                    // Add new item
                    // Find max sort order
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
    }

    if changed {
        state.dirty = true;
    }
}

/// Render clickable toolbar preview - clicking selects an item
fn render_clickable_preview(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
) {
    egui::Frame::NONE
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .inner_margin(spacing::CARD)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

                // Get visible items sorted by sort_order
                let mut visible_items: Vec<(usize, i32)> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.visible)
                    .map(|(idx, i)| (idx, i.sort_order))
                    .collect();
                visible_items.sort_by_key(|(_, order)| *order);

                // Track groups for separators
                let mut last_group: Option<String> = None;

                for (item_idx, _) in visible_items {
                    let item = &items[item_idx];
                    let is_selected = selected_id.as_ref() == Some(&item.id);

                    // Add separator between groups
                    if last_group.is_some() && last_group != item.group_id {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    last_group = item.group_id.clone();

                    let icon = item
                        .icon
                        .as_ref()
                        .map(|n| icon_name_to_char(n))
                        .unwrap_or("");

                    let btn_text = match item.display_mode {
                        DisplayMode::IconOnly => icon.to_string(),
                        DisplayMode::TextOnly => item.label.clone(),
                        DisplayMode::IconAndText => format!("{} {}", icon, item.label),
                    };

                    // Different styling for selected item
                    let fill = if is_selected {
                        theme.colors.primary_container
                    } else {
                        theme.colors.surface
                    };

                    let stroke = if is_selected {
                        egui::Stroke::new(2.0, theme.colors.primary)
                    } else {
                        egui::Stroke::NONE
                    };

                    let btn = ui.add(
                        egui::Button::new(egui::RichText::new(&btn_text).size(14.0))
                            .fill(fill)
                            .stroke(stroke)
                            .min_size(egui::vec2(32.0, 32.0)),
                    );

                    if btn.clicked() {
                        if is_selected {
                            // Clicking selected item deselects
                            *selected_id = None;
                        } else {
                            // Select this item
                            *selected_id = Some(item.id.clone());
                        }
                    }
                }
            });
        });
}

/// Render selection area with [←] [Item] [→] [Remove] [Done]
fn render_selection_area(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    let Some(sel_id) = selected_id.clone() else {
        return;
    };

    // Find selected item index and its sort position
    let sel_idx = items.iter().position(|i| i.id == sel_id);
    let Some(sel_idx) = sel_idx else {
        *selected_id = None;
        return;
    };

    // Get visible items sorted by sort_order to find neighbors
    let mut visible_sorted: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.visible)
        .map(|(idx, i)| (idx, i.sort_order))
        .collect();
    visible_sorted.sort_by_key(|(_, order)| *order);

    // Find position of selected item in visible list
    let vis_pos = visible_sorted.iter().position(|(idx, _)| *idx == sel_idx);
    let Some(vis_pos) = vis_pos else {
        *selected_id = None;
        return;
    };

    let can_move_left = vis_pos > 0;
    let can_move_right = vis_pos < visible_sorted.len() - 1;

    // Copy data we need before potentially mutating items
    let icon = items[sel_idx]
        .icon
        .as_ref()
        .map(|n| icon_name_to_char(n))
        .unwrap_or("");
    let item_label = items[sel_idx].label.clone();

    // Centered selection area using horizontal with centered layout
    ui.horizontal(|ui| {
        // Add flexible space before to push content to center
        ui.add_space(ui.available_width() / 2.0 - 200.0); // Approximate centering

        ui.horizontal(|ui| {
            // Move Left button
            let left_btn = ui.add_enabled(
                can_move_left,
                egui::Button::new(
                    egui::RichText::new(egui_phosphor::regular::ARROW_LEFT)
                        .color(theme.colors.on_surface),
                )
                .min_size(egui::vec2(36.0, 36.0)),
            );
            if left_btn.clicked() && can_move_left {
                // Swap with previous visible item
                let prev_idx = visible_sorted[vis_pos - 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[prev_idx].sort_order;
                items[prev_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(8.0);

            // Selected item label with proper text color
            ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("{} {}", icon, item_label))
                        .size(16.0)
                        .strong()
                        .color(theme.colors.on_primary_container),
                )
                .fill(theme.colors.primary_container)
                .min_size(egui::vec2(100.0, 36.0)),
            );

            ui.add_space(8.0);

            // Move Right button
            let right_btn = ui.add_enabled(
                can_move_right,
                egui::Button::new(
                    egui::RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                        .color(theme.colors.on_surface),
                )
                .min_size(egui::vec2(36.0, 36.0)),
            );
            if right_btn.clicked() && can_move_right {
                // Swap with next visible item
                let next_idx = visible_sorted[vis_pos + 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[next_idx].sort_order;
                items[next_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(24.0);

            // Remove button (hide from toolbar)
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Remove", egui_phosphor::regular::TRASH))
                        .color(theme.colors.on_surface),
                ))
                .clicked()
            {
                items[sel_idx].visible = false;
                *selected_id = None;
                *dirty = true;
            }

            ui.add_space(8.0);

            // Done button (deselect)
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("Done").color(theme.colors.on_surface),
                ))
                .clicked()
            {
                *selected_id = None;
            }
        });
    });
}

/// Render item picker grid - click to toggle visibility or select
fn render_item_picker(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    let groups = ["navigation", "file_actions", "view", "panels", "plugins"];

    for group_name in groups {
        let pretty_name = match group_name {
            "navigation" => "Navigation",
            "file_actions" => "File Actions",
            "view" => "View Mode",
            "panels" => "Panel Toggles",
            "plugins" => "Plugins",
            _ => group_name,
        };

        // Get items in this group
        let group_indices: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.group_id.as_deref() == Some(group_name))
            .map(|(idx, _)| idx)
            .collect();

        if group_indices.is_empty() {
            continue;
        }

        ui.label(
            egui::RichText::new(pretty_name)
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            for &item_idx in &group_indices {
                let item = &items[item_idx];
                let is_selected = selected_id.as_ref() == Some(&item.id);
                let icon = item
                    .icon
                    .as_ref()
                    .map(|n| icon_name_to_char(n))
                    .unwrap_or("");

                let response = SelectableChip::new(&item.label)
                    .icon(icon)
                    .selected(is_selected)
                    .active(item.visible)
                    .with_theme_colors(&theme.colors)
                    .show(ui);

                // Click behavior: simple toggle visibility
                // - If visible: hide it
                // - If hidden: show it
                if response.clicked() {
                    if items[item_idx].visible {
                        // Hide it - also deselect if this was selected
                        if is_selected {
                            *selected_id = None;
                        }
                        items[item_idx].visible = false;
                    } else {
                        // Show it
                        items[item_idx].visible = true;
                    }
                    *dirty = true;
                }
            }
        });

        ui.add_space(12.0);
    }
}

/// Convert icon name to phosphor icon character
fn icon_name_to_char(name: &str) -> &'static str {
    match name {
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
    }
}
