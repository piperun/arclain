//! Shared rendering helpers for the layout editor.
//!
//! Pure UI code: takes the already-loaded item list + selection state
//! and renders the three-section editor layout (preview → selection
//! area → picker). No service or DB access — all data manipulation
//! happens against the in-memory `items` slice; persistence flows back
//! through the dispatcher.
//!
//! Axis-dependent parts (preview layout, arrow directions, picker
//! grouping) branch on `Region` trait constants so the same render
//! function serves both toolbar and info panel.

use super::editor::{Axis, LayoutEditorAction, LayoutEditorState, Region};
use crate::shared::theme::AppTheme;
use arclain_app::layout::{UiDisplayModeDto, UiItemDto};
use arclain_theme::spacing;
use arclain_widgets::SelectableChip;
use eframe::egui;

/// Render the full layout-editor page. Auto-fires a `SyncItems`
/// action so the parent dispatches initial load + per-frame plugin
/// reconciliation.
pub fn render_layout_editor<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut LayoutEditorState<R>,
) -> Option<LayoutEditorAction> {
    // Section: Live Preview (click to select).
    let preview_heading = match R::AXIS {
        Axis::Horizontal => "Toolbar Preview (click item to select)",
        Axis::Vertical => "Panel Sections (click to select, drag to reorder)",
    };
    ui.label(
        egui::RichText::new(preview_heading)
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_preview::<R>(ui, theme, &mut state.items, &mut state.selected_item_id);

    ui.add_space(16.0);

    // Selection area: shown only when something is selected.
    if state.selected_item_id.is_some() {
        render_selection_area::<R>(
            ui,
            theme,
            &mut state.items,
            &mut state.selected_item_id,
            &mut state.dirty,
        );
        ui.add_space(16.0);
    }

    // Section: Available items picker.
    let picker_heading = match R::AXIS {
        Axis::Horizontal => "Available Items (click to show/hide)",
        Axis::Vertical => "Available Sections (click to show/hide)",
    };
    ui.label(
        egui::RichText::new(picker_heading)
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    render_picker::<R>(
        ui,
        theme,
        &mut state.items,
        &mut state.selected_item_id,
        &mut state.dirty,
    );

    Some(LayoutEditorAction::SyncItems)
}

fn render_preview<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
) {
    match R::AXIS {
        Axis::Horizontal => render_horizontal_preview::<R>(ui, theme, items, selected_id),
        Axis::Vertical => render_vertical_preview(ui, theme, items, selected_id),
    }
}

fn render_horizontal_preview<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
) {
    egui::Frame::NONE
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::new(1.0_f32, theme.colors.outline))
        .inner_margin(spacing::CARD)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

                let mut visible_items: Vec<(usize, i32)> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.visible)
                    .map(|(idx, i)| (idx, i.sort_order))
                    .collect();
                visible_items.sort_by_key(|(_, order)| *order);

                let mut last_group: Option<String> = None;

                for (item_idx, _) in visible_items {
                    let item = &items[item_idx];
                    let is_selected = selected_id.as_ref() == Some(&item.id);

                    if last_group.is_some() && last_group != item.group_id {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                    last_group = item.group_id.clone();

                    let icon = item
                        .icon
                        .as_deref()
                        .and_then(R::icon_for_name)
                        .unwrap_or("");

                    let btn_text = match item.display_mode {
                        UiDisplayModeDto::IconOnly => icon.to_string(),
                        UiDisplayModeDto::TextOnly => item.label.clone(),
                        UiDisplayModeDto::IconAndText => format!("{} {}", icon, item.label),
                    };

                    let fill = if is_selected {
                        theme.colors.primary_container
                    } else {
                        theme.colors.surface
                    };
                    let stroke = if is_selected {
                        egui::Stroke::new(2.0_f32, theme.colors.primary)
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
                            *selected_id = None;
                        } else {
                            *selected_id = Some(item.id.clone());
                        }
                    }
                }
            });
        });
}

fn render_vertical_preview(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
) {
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

    ui.vertical_centered(|ui| {
        egui::Frame::NONE
            .fill(theme.colors.surface_variant)
            .stroke(egui::Stroke::new(1.0_f32, theme.colors.outline))
            .inner_margin(spacing::CARD)
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
                                if is_selected { 2.0_f32 } else { 1.0_f32 },
                                if is_selected {
                                    theme.colors.primary
                                } else {
                                    theme.colors.outline
                                },
                            ))
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new(&item.label)
                                            .size(14.0)
                                            .color(text_color),
                                    );
                                });
                            });

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

fn render_selection_area<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    let Some(sel_id) = selected_id.clone() else {
        return;
    };

    let Some(sel_idx) = items.iter().position(|i| i.id == sel_id) else {
        *selected_id = None;
        return;
    };

    let mut visible_sorted: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.visible)
        .map(|(idx, i)| (idx, i.sort_order))
        .collect();
    visible_sorted.sort_by_key(|(_, order)| *order);

    let Some(vis_pos) = visible_sorted.iter().position(|(idx, _)| *idx == sel_idx) else {
        *selected_id = None;
        return;
    };

    let can_move_prev = vis_pos > 0;
    let can_move_next = vis_pos < visible_sorted.len() - 1;

    let icon = items[sel_idx]
        .icon
        .as_deref()
        .and_then(R::icon_for_name)
        .unwrap_or("");
    let item_label = items[sel_idx].label.clone();

    let (prev_glyph, next_glyph) = match R::AXIS {
        Axis::Horizontal => (
            egui_phosphor::regular::ARROW_LEFT,
            egui_phosphor::regular::ARROW_RIGHT,
        ),
        Axis::Vertical => (
            egui_phosphor::regular::ARROW_UP,
            egui_phosphor::regular::ARROW_DOWN,
        ),
    };

    // Approximate horizontal centering. Width 200 covers the typical
    // toolbar selection area (left arrow + label + right arrow +
    // remove + done) — slightly different for the vertical case but
    // close enough that the user reads it as centered.
    ui.horizontal(|ui| {
        // Clamped: narrower than the toolbar itself, the offset goes
        // negative and walks the cursor back over what came before.
        ui.add_space((ui.available_width() / 2.0 - 200.0).max(0.0));

        ui.horizontal(|ui| {
            let prev_btn = ui.add_enabled(
                can_move_prev,
                egui::Button::new(egui::RichText::new(prev_glyph).color(theme.colors.on_surface))
                    .min_size(egui::vec2(36.0, 36.0)),
            );
            if prev_btn.clicked() && can_move_prev {
                let prev_idx = visible_sorted[vis_pos - 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[prev_idx].sort_order;
                items[prev_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(8.0);

            // Selected item label. Horizontal layouts include the icon;
            // vertical (info panel) regions return None from
            // `icon_for_name` so the label rendering is text-only.
            let label_text = if icon.is_empty() {
                item_label.clone()
            } else {
                format!("{} {}", icon, item_label)
            };
            ui.add(
                egui::Button::new(
                    egui::RichText::new(label_text)
                        .size(16.0)
                        .strong()
                        .color(theme.colors.on_primary_container),
                )
                .fill(theme.colors.primary_container)
                .min_size(egui::vec2(100.0, 36.0)),
            );

            ui.add_space(8.0);

            let next_btn = ui.add_enabled(
                can_move_next,
                egui::Button::new(egui::RichText::new(next_glyph).color(theme.colors.on_surface))
                    .min_size(egui::vec2(36.0, 36.0)),
            );
            if next_btn.clicked() && can_move_next {
                let next_idx = visible_sorted[vis_pos + 1].0;
                let tmp = items[sel_idx].sort_order;
                items[sel_idx].sort_order = items[next_idx].sort_order;
                items[next_idx].sort_order = tmp;
                *dirty = true;
            }

            ui.add_space(24.0);

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

fn render_picker<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    let groups = R::picker_groups();
    if groups.is_empty() {
        render_flat_picker::<R>(ui, theme, items, selected_id, dirty);
    } else {
        render_grouped_picker::<R>(ui, theme, items, selected_id, dirty, groups);
    }
}

fn render_flat_picker<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

        for item_idx in 0..items.len() {
            let item = &items[item_idx];
            let is_selected = selected_id.as_ref() == Some(&item.id);
            let icon = item.icon.as_deref().and_then(R::icon_for_name);

            let mut chip = SelectableChip::new(&items[item_idx].label)
                .selected(is_selected)
                .active(item.visible)
                .with_theme_colors(&theme.colors);
            if let Some(ic) = icon {
                chip = chip.icon(ic);
            }
            let response = chip.show(ui);

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

        // Renormalize sort_order across visible items after a toggle, so
        // newly-visible items land at the end with a stable ordering.
        // Matches the pre-MVU info-panel behavior; toolbars use sparse
        // sort_order values so this only takes effect for flat pickers.
        if *dirty {
            let mut visible_items: Vec<&mut UiItemDto> =
                items.iter_mut().filter(|i| i.visible).collect();
            visible_items.sort_by_key(|i| i.sort_order);
            for (idx, item) in visible_items.iter_mut().enumerate() {
                item.sort_order = idx as i32;
            }
        }
    });
}

fn render_grouped_picker<R: Region>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &mut [UiItemDto],
    selected_id: &mut Option<String>,
    dirty: &mut bool,
    groups: &'static [(&'static str, &'static str)],
) {
    for (group_name, pretty_name) in groups {
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
            egui::RichText::new(*pretty_name)
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

            for &item_idx in &group_indices {
                let item = &items[item_idx];
                let is_selected = selected_id.as_ref() == Some(&item.id);
                let icon = item.icon.as_deref().and_then(R::icon_for_name);

                let mut chip = SelectableChip::new(&item.label)
                    .selected(is_selected)
                    .active(item.visible)
                    .with_theme_colors(&theme.colors);
                if let Some(ic) = icon {
                    chip = chip.icon(ic);
                }
                let response = chip.show(ui);

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

        ui.add_space(12.0);
    }
}
