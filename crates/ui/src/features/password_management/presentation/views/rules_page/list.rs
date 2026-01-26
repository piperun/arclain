use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::theme::AppTheme;
use arclain_widgets::toggle_switch::ToggleSwitch;
use eframe::egui;

pub fn render_list(ui: &mut egui::Ui, theme: &AppTheme, dialog: &mut PasswordRulesDialog) {
    // ---------------------------------------------------------
    // Section 2: Rule Registry (List)
    // ---------------------------------------------------------

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Rule Registry")
                //.size(15.0)
                .strong()
                .color(theme.colors.on_surface),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} rules", dialog.rules.len()))
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
        });
    });
    ui.add_space(4.0);

    // Logic for list actions
    let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

    // Define columns
    let columns = vec![
        TableColumn::exact(60.0, "Enabled"),
        TableColumn::resizable(180.0, "Name"),
        TableColumn::remainder("Pattern"),
        TableColumn::exact(60.0, "Pri"),
        TableColumn::exact(90.0, "Actions").align_right(),
    ];

    // Render table using standardized ItemTable component
    let actions = ItemTable::new()
        .empty_message("No password rules configured yet.")
        .show(ui, theme, &columns, &dialog.rules, |rule, idx, row, actions| {
            // Enabled column
            row.col(|ui| {
                ui.centered_and_justified(|ui| {
                    let mut enabled = rule.enabled;
                    if ui
                        .add(
                            ToggleSwitch::new(&mut enabled)
                                .text("ON", "OFF")
                                .size(40.0, 18.0)
                                .with_theme_colors(&theme.colors),
                        )
                        .changed()
                    {
                        enable_toggles.push((idx, enabled));
                    }
                });
            });

            // Name column
            row.col(|ui| {
                ui.label(egui::RichText::new(&rule.name).color(
                    if rule.enabled {
                        theme.colors.on_surface
                    } else {
                        theme.colors.on_surface_variant
                    },
                ));
            });

            // Pattern column
            row.col(|ui| {
                ui.label(
                    egui::RichText::new(&rule.pattern)
                        .family(egui::FontFamily::Monospace)
                        .color(theme.colors.on_surface_variant),
                );
            });

            // Priority column
            row.col(|ui| {
                ui.label(
                    egui::RichText::new(rule.priority.to_string())
                        .color(theme.colors.on_surface_variant),
                );
            });

            // Actions column
            row.col(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{}", egui_phosphor::regular::TRASH))
                        .on_hover_text("Delete rule")
                        .clicked()
                    {
                        actions.delete(idx);
                    }
                    ui.add_space(8.0);

                    let is_editing_this = Some(idx) == dialog.editing_index;
                    if ui
                        .add_enabled(
                            !is_editing_this,
                            egui::Button::new(format!("{}", egui_phosphor::regular::PENCIL)),
                        )
                        .on_hover_text("Edit rule")
                        .clicked()
                    {
                        actions.edit(idx);
                    }
                });
            });
        });

    // Apply deferred actions after immutable borrow ends
    if let Some(idx) = actions.get_delete() {
        dialog.rules.remove(*idx);
        // If we deleted the one we were editing, cancel edit
        if Some(*idx) == dialog.editing_index {
            dialog.editing_index = None;
            dialog.edit_name.clear();
            dialog.edit_pattern.clear();
            dialog.edit_password.clear();
            dialog.edit_priority = "10".to_string();
            dialog.edit_enabled = true;
        } else if let Some(edit_idx) = dialog.editing_index {
            // Shift index if needed
            if *idx < edit_idx {
                dialog.editing_index = Some(edit_idx - 1);
            }
        }
    }

    if let Some(idx) = actions.get_edit() {
        if let Some(rule) = dialog.rules.get(*idx) {
            dialog.editing_index = Some(*idx);
            dialog.edit_name = rule.name.clone();
            dialog.edit_pattern = rule.pattern.clone();
            dialog.edit_password = rule.password.clone();
            dialog.edit_priority = rule.priority.to_string();
            dialog.edit_enabled = rule.enabled;
        }
    }

    // Handle enable/disable toggles
    for (idx, enabled) in enable_toggles {
        if let Some(r) = dialog.rules.get_mut(idx) {
            r.enabled = enabled;
            // Sync with edit form if we are currently editing THIS rule
            if Some(idx) == dialog.editing_index {
                dialog.edit_enabled = enabled;
            }
        }
    }
}
