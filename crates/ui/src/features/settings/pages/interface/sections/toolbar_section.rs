//! Toolbar settings section - configure visibility, order, and display mode of toolbar buttons.

use crate::shared::theme::AppTheme;
use arclain_db::{DisplayMode, UiItem, UiRegion};
use arclain_widgets::{CollapsibleSection, ToggleSwitch};
use eframe::egui;

/// Render the toolbar configuration section
pub fn render(ui: &mut egui::Ui, theme: &AppTheme, items: &mut Vec<UiItem>, on_change: &mut bool) {
    ui.label(
        egui::RichText::new("Configure which buttons appear in the toolbar and how they display")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);

    // Group items by group_id
    let mut groups: std::collections::BTreeMap<String, Vec<&mut UiItem>> =
        std::collections::BTreeMap::new();

    for item in items.iter_mut() {
        if item.region == UiRegion::Toolbar {
            let group_name = item.group_id.clone().unwrap_or_else(|| "Other".to_string());
            groups.entry(group_name).or_default().push(item);
        }
    }

    // Render each group
    for (group_name, mut group_items) in groups {
        render_group(ui, theme, &group_name, &mut group_items, on_change);
        ui.add_space(8.0);
    }
}

fn render_group(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    group_name: &str,
    items: &mut [&mut UiItem],
    on_change: &mut bool,
) {
    // Sort by sort_order
    items.sort_by_key(|i| i.sort_order);

    let pretty_name = match group_name {
        "navigation" => "Navigation",
        "file_actions" => "File Actions",
        "view" => "View Mode",
        "panels" => "Panel Toggles",
        _ => group_name,
    };

    // Track reorder action (index, direction: -1 up, +1 down)
    let mut reorder_action: Option<(usize, i32)> = None;

    CollapsibleSection::new(group_name, pretty_name)
        .default_open(true)
        .with_theme_colors(&theme.colors)
        .show(ui, |ui| {
            ui.add_space(4.0);

            let item_count = items.len();
            for (idx, item) in items.iter_mut().enumerate() {
                let icon = item
                    .icon
                    .as_ref()
                    .map(|n| icon_name_to_char(n))
                    .unwrap_or("");
                let label = format!("{} {}", icon, item.label);

                // Custom row with reorder buttons
                ui.horizontal(|ui| {
                    // Reorder buttons (sharp Y2K style)
                    ui.vertical(|ui| {
                        ui.set_width(24.0);
                        ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);

                        // Up button
                        let up_enabled = idx > 0;
                        if ui
                            .add_enabled(
                                up_enabled,
                                egui::Button::new(egui_phosphor::regular::CARET_UP).small(),
                            )
                            .clicked()
                        {
                            reorder_action = Some((idx, -1));
                        }

                        // Down button
                        let down_enabled = idx < item_count - 1;
                        if ui
                            .add_enabled(
                                down_enabled,
                                egui::Button::new(egui_phosphor::regular::CARET_DOWN).small(),
                            )
                            .clicked()
                        {
                            reorder_action = Some((idx, 1));
                        }
                    });

                    ui.add_space(4.0);

                    // Order number display
                    ui.label(
                        egui::RichText::new(format!("#{}", idx + 1))
                            .small()
                            .color(theme.colors.on_surface_variant),
                    );

                    ui.add_space(8.0);

                    // Item label
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&label)
                                .strong()
                                .color(theme.colors.on_surface),
                        );
                        ui.label(
                            egui::RichText::new(display_mode_description(item.display_mode))
                                .small()
                                .color(theme.colors.on_surface_variant),
                        );
                    });

                    // Spacer
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Visibility toggle
                        if ui
                            .add(ToggleSwitch::new(&mut item.visible).size(44.0, 22.0))
                            .changed()
                        {
                            *on_change = true;
                        }

                        ui.add_space(8.0);

                        // Display mode dropdown
                        let current_mode = match item.display_mode {
                            DisplayMode::IconAndText => "Icon+Text",
                            DisplayMode::IconOnly => "Icon",
                            DisplayMode::TextOnly => "Text",
                        };

                        egui::ComboBox::from_id_salt(&item.id)
                            .selected_text(current_mode)
                            .width(80.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        item.display_mode == DisplayMode::IconAndText,
                                        "Icon+Text",
                                    )
                                    .clicked()
                                {
                                    item.display_mode = DisplayMode::IconAndText;
                                    *on_change = true;
                                }
                                if ui
                                    .selectable_label(
                                        item.display_mode == DisplayMode::IconOnly,
                                        "Icon",
                                    )
                                    .clicked()
                                {
                                    item.display_mode = DisplayMode::IconOnly;
                                    *on_change = true;
                                }
                                if ui
                                    .selectable_label(
                                        item.display_mode == DisplayMode::TextOnly,
                                        "Text",
                                    )
                                    .clicked()
                                {
                                    item.display_mode = DisplayMode::TextOnly;
                                    *on_change = true;
                                }
                            });
                    });
                });

                ui.add_space(4.0);
            }
        });

    // Apply reorder action after rendering
    if let Some((idx, direction)) = reorder_action {
        let new_idx = (idx as i32 + direction) as usize;
        if new_idx < items.len() {
            // Swap sort_order values
            let order_a = items[idx].sort_order;
            let order_b = items[new_idx].sort_order;
            tracing::info!(
                "Reordering items: idx={} ({}) sort_order {} <-> idx={} ({}) sort_order {}",
                idx,
                items[idx].id,
                order_a,
                new_idx,
                items[new_idx].id,
                order_b
            );
            items[idx].sort_order = order_b;
            items[new_idx].sort_order = order_a;
            *on_change = true;
        }
    }
}

fn display_mode_description(mode: DisplayMode) -> String {
    match mode {
        DisplayMode::IconAndText => "Shows icon and label".to_string(),
        DisplayMode::IconOnly => "Shows icon only".to_string(),
        DisplayMode::TextOnly => "Shows text only".to_string(),
    }
}

/// Map icon name to Phosphor icon character
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
        "COPY" => egui_phosphor::regular::COPY,
        _ => egui_phosphor::regular::QUESTION,
    }
}
