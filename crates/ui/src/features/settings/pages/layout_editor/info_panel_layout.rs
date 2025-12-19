//! Info Panel Layout Editor
//!
//! Visual editor for customizing info panel section layout with:
//! - Live preview of panel sections with click-to-select
//! - Selection area with move up/down arrows
//! - Section list for toggling visibility

use crate::shared::theme::AppTheme;
use arclain_db::{list_items_by_region, upsert_item, UiItem, UiRegion};
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginExtensionPoint;
use eframe::egui;

/// State for info panel layout editor
pub struct InfoPanelLayoutState {
    pub items: Vec<UiItem>,
    pub loaded: bool,
    pub dirty: bool,
    pub selected_item_id: Option<String>,
}

impl Default for InfoPanelLayoutState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            loaded: false,
            dirty: false,
            selected_item_id: None,
        }
    }
}

impl InfoPanelLayoutState {
    pub fn load_from_db(&mut self, conn: &rusqlite::Connection) {
        if let Ok(items) = list_items_by_region(conn, UiRegion::InfoPanel) {
            self.items = items;
            self.items.sort_by_key(|i| i.sort_order);
            self.loaded = true;
            self.dirty = false;
            self.selected_item_id = None;
        }
    }

    pub fn save_to_db(&mut self, conn: &rusqlite::Connection) {
        for item in &self.items {
            let _ = upsert_item(conn, item);
        }
        self.dirty = false;
    }
}

/// Render the Info Panel Layout Editor page
pub fn render_info_panel_layout(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    state: &mut InfoPanelLayoutState,
    plugin_manager: Option<&PluginManager>,
) {
    // Load items from database
    if !state.loaded {
        let state_guard = app_state.lock();
        if let Some(dbs) = &state_guard.dbs {
            let _ = dbs.config.with_connection(|conn| {
                state.load_from_db(conn);
                Ok::<_, anyhow::Error>(())
            });
        }
    }

    // The header with Save/Reset is now rendered by SettingsHeader in ui.rs
    // This page only renders the content area

    // Section: Live Preview (click to select)
    ui.label(
        egui::RichText::new("Panel Sections (click to select, drag to reorder)")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_section_preview(ui, theme, &mut state.items, &mut state.selected_item_id);

    // Selection area (only shown when something is selected)
    if state.selected_item_id.is_some() {
        ui.add_space(8.0);
        render_selection_area(
            ui,
            theme,
            &mut state.items,
            &mut state.selected_item_id,
            &mut state.dirty,
        );
        ui.add_space(16.0);
    }

    ui.add_space(16.0);

    // Section: Available Sections (click to toggle visibility)
    ui.label(
        egui::RichText::new("Available Sections (click to show/hide)")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_section_picker(
        ui,
        theme,
        &mut state.items,
        &mut state.selected_item_id,
        &mut state.dirty,
    );

    // Plugin info panel items section
    if let Some(manager) = plugin_manager {
        render_plugin_info_panel_section(ui, theme, manager);
    }
}

/// Render clickable panel section preview - vertical list of sections
fn render_section_preview(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
) {
    // Get visible sections sorted by sort_order
    let mut visible_items: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.visible)
        .map(|(idx, i)| (idx, i.sort_order))
        .collect();
    visible_items.sort_by_key(|(_, order)| *order);

    if visible_items.is_empty() {
        ui.label(
            egui::RichText::new("No sections visible. Click below to add sections.")
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
        return;
    }

    // Centered preview frame
    ui.vertical_centered(|ui| {
        egui::Frame::NONE
            .fill(theme.colors.surface_variant)
            .stroke(egui::Stroke::new(1.0, theme.colors.outline))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(300.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

                    for (original_idx, _) in &visible_items {
                        let item = &items[*original_idx];
                        let is_selected = selected_id.as_ref() == Some(&item.id);

                        let bg = if is_selected {
                            theme.colors.primary_container
                        } else {
                            theme.colors.surface
                        };
                        let text_color = if is_selected {
                            theme.colors.on_primary_container
                        } else {
                            theme.colors.on_surface
                        };

                        let section_btn = egui::Frame::NONE
                            .fill(bg)
                            .stroke(egui::Stroke::new(
                                if is_selected { 2.0 } else { 1.0 },
                                if is_selected {
                                    theme.colors.primary
                                } else {
                                    theme.colors.outline
                                },
                            ))
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new(&item.label)
                                        .size(14.0)
                                        .color(text_color),
                                );
                            });

                        // Make clickable
                        let response = ui.interact(
                            section_btn.response.rect,
                            ui.id().with(format!("section_{}", original_idx)),
                            egui::Sense::click(),
                        );

                        if response.clicked() {
                            let item_id = items[*original_idx].id.clone();
                            if is_selected {
                                *selected_id = None;
                            } else {
                                *selected_id = Some(item_id);
                            }
                        }
                    }
                });
            });
    });
}

/// Render selection area with move up/down, remove, done buttons
fn render_selection_area(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    let sel_id = match selected_id {
        Some(id) => id.clone(),
        None => return,
    };

    let sel_idx = match items.iter().position(|i| i.id == sel_id) {
        Some(idx) => idx,
        None => {
            *selected_id = None;
            return;
        }
    };

    // Get visible items sorted for position calculation
    let mut visible_sorted: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.visible)
        .map(|(idx, i)| (idx, i.sort_order))
        .collect();
    visible_sorted.sort_by_key(|(_, order)| *order);

    let vis_pos = visible_sorted.iter().position(|(idx, _)| *idx == sel_idx);
    let can_move_up = vis_pos.map(|p| p > 0).unwrap_or(false);
    let can_move_down = vis_pos
        .map(|p| p < visible_sorted.len() - 1)
        .unwrap_or(false);
    let vis_pos = vis_pos.unwrap_or(0);

    let item_label = items[sel_idx].label.clone();

    // Centered selection area
    ui.horizontal(|ui| {
        ui.add_space(ui.available_width() / 2.0 - 180.0);

        ui.horizontal(|ui| {
            // Move Up button
            let up_btn = ui.add_enabled(
                can_move_up,
                egui::Button::new(
                    egui::RichText::new(egui_phosphor::regular::ARROW_UP)
                        .color(theme.colors.on_surface),
                )
                .min_size(egui::vec2(36.0, 36.0)),
            );
            if up_btn.clicked() && can_move_up {
                let prev_idx = visible_sorted[vis_pos - 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[prev_idx].sort_order;
                items[prev_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(8.0);

            // Selected item label
            ui.add(
                egui::Button::new(
                    egui::RichText::new(&item_label)
                        .size(16.0)
                        .strong()
                        .color(theme.colors.on_primary_container),
                )
                .fill(theme.colors.primary_container)
                .min_size(egui::vec2(120.0, 36.0)),
            );

            ui.add_space(8.0);

            // Move Down button
            let down_btn = ui.add_enabled(
                can_move_down,
                egui::Button::new(
                    egui::RichText::new(egui_phosphor::regular::ARROW_DOWN)
                        .color(theme.colors.on_surface),
                )
                .min_size(egui::vec2(36.0, 36.0)),
            );
            if down_btn.clicked() && can_move_down {
                let next_idx = visible_sorted[vis_pos + 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[next_idx].sort_order;
                items[next_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(24.0);

            // Remove button
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

            // Done button
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

/// Render section picker - click to toggle visibility
fn render_section_picker(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItem],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

        for item_idx in 0..items.len() {
            let item = &items[item_idx];
            let is_selected = selected_id.as_ref() == Some(&item.id);

            // Chip styling based on visibility and selection
            let (bg, text_color, stroke) = if is_selected {
                (
                    theme.colors.primary,
                    theme.colors.on_primary,
                    egui::Stroke::new(2.0, theme.colors.primary),
                )
            } else if item.visible {
                (
                    theme.colors.primary_container,
                    theme.colors.on_primary_container,
                    egui::Stroke::new(1.0, theme.colors.outline),
                )
            } else {
                (
                    theme.colors.surface_variant.gamma_multiply(0.7),
                    theme.colors.on_surface_variant,
                    egui::Stroke::new(1.0, theme.colors.outline.gamma_multiply(0.5)),
                )
            };

            let chip = egui::Frame::NONE
                .fill(bg)
                .stroke(stroke)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&items[item_idx].label).color(text_color));
                });

            // Make the frame interactive
            let response = ui.interact(
                chip.response.rect,
                ui.id().with(format!("info_chip_{}", item_idx)),
                egui::Sense::click(),
            );

            // Click behavior: simple toggle visibility
            if response.clicked() {
                if items[item_idx].visible {
                    if is_selected {
                        *selected_id = None;
                    }
                    items[item_idx].visible = false;
                } else {
                    items[item_idx].visible = true;
                }
                *dirty = true;
            }
        }
    });
}

/// Render section showing plugins that provide Info Panel UI
fn render_plugin_info_panel_section(ui: &mut egui::Ui, theme: &AppTheme, manager: &PluginManager) {
    let enabled_plugins: Vec<_> = manager
        .list_plugins()
        .iter()
        .filter(|p| p.enabled)
        .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
        .collect();

    if enabled_plugins.is_empty() {
        return;
    }

    // Check which plugins provide InfoPanel UI
    let mut info_panel_plugins: Vec<(String, String)> = Vec::new();
    for (plugin_id, plugin_name) in &enabled_plugins {
        let has_info_panel = manager.with_plugin_instance(plugin_id, |instance| {
            if let Ok(elements) = instance.get_ui_layout(PluginExtensionPoint::InfoPanel) {
                !elements.is_empty()
            } else {
                false
            }
        });
        if has_info_panel == Some(true) {
            info_panel_plugins.push((plugin_id.clone(), plugin_name.clone()));
        }
    }

    if info_panel_plugins.is_empty() {
        return;
    }

    ui.add_space(16.0);

    // Render the plugin section
    ui.label(
        egui::RichText::new("Plugin Info Panel Sections (auto-inserted)")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

        for (plugin_id, plugin_name) in &info_panel_plugins {
            let chip = egui::Frame::NONE
                .fill(theme.colors.tertiary_container)
                .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}",
                            egui_phosphor::regular::PUZZLE_PIECE,
                            plugin_name
                        ))
                        .color(theme.colors.on_tertiary_container),
                    );
                });

            // Tooltip on hover
            chip.response.on_hover_ui(|ui| {
                ui.label(format!("Plugin: {}", plugin_id));
                ui.label("This plugin provides info panel sections.");
                ui.label("Enable/disable the plugin in Plugins settings.");
            });
        }
    });
}
