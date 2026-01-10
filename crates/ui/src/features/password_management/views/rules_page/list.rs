use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
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
    let mut to_delete: Option<usize> = None;
    let mut to_edit: Option<usize> = None;
    let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

    if dialog.rules.is_empty() {
        egui::Frame::NONE
            .fill(theme.colors.surface_variant)
            .stroke(egui::Stroke::new(1.0, theme.colors.outline))
            .corner_radius(8.0)
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("No password rules configured yet.")
                            .size(13.0)
                            .color(theme.colors.on_surface_variant),
                    );
                });
            });
    } else {
        // Table container using TableBuilder
        egui::Frame::NONE
            .fill(theme.colors.secondary) // Darker background for contrast
            .stroke(egui::Stroke::new(1.0, theme.colors.outline))
            .corner_radius(8.0)
            .inner_margin(4.0) // Add some padding inside the frame
            .show(ui, |ui| {
                use egui_extras::{Column, TableBuilder};

                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(60.0)) // Enabled
                    .column(Column::initial(180.0).resizable(true)) // Name
                    .column(Column::remainder().clip(true)) // Pattern (Takes remainder, prevents overflow)
                    .column(Column::exact(60.0)) // Priority
                    .column(Column::exact(90.0)) // Actions (Wider)
                    .min_scrolled_height(0.0)
                    .header(24.0, |mut header| {
                        header.col(|ui| {
                            ui.label(
                                egui::RichText::new("Enabled")
                                    .strong()
                                    .color(theme.colors.on_surface),
                            );
                        });
                        header.col(|ui| {
                            ui.label(
                                egui::RichText::new("Name")
                                    .strong()
                                    .color(theme.colors.on_surface),
                            );
                        });
                        header.col(|ui| {
                            ui.label(
                                egui::RichText::new("Pattern")
                                    .strong()
                                    .color(theme.colors.on_surface),
                            );
                        });
                        header.col(|ui| {
                            ui.label(
                                egui::RichText::new("Pri")
                                    .strong()
                                    .color(theme.colors.on_surface),
                            );
                        });
                        header.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("Actions")
                                            .strong()
                                            .color(theme.colors.on_surface),
                                    );
                                },
                            );
                        });
                    })
                    .body(|mut body| {
                        for (idx, rule) in dialog.rules.iter().enumerate() {
                            body.row(30.0, |mut row| {
                                // Slightly taller rows
                                row.col(|ui| {
                                    ui.centered_and_justified(|ui| {
                                        let mut enabled = rule.enabled;
                                        if ui
                                            .add(
                                                ToggleSwitch::new(&mut enabled)
                                                    .text("ON", "OFF")
                                                    .size(40.0, 18.0),
                                            )
                                            .changed()
                                        {
                                            enable_toggles.push((idx, enabled));
                                        }
                                    });
                                });
                                row.col(|ui| {
                                    ui.label(egui::RichText::new(&rule.name).color(
                                        if rule.enabled {
                                            theme.colors.on_surface
                                        } else {
                                            theme.colors.on_surface_variant
                                            // Use muted for disabled
                                        },
                                    ));
                                });
                                row.col(|ui| {
                                    ui.label(
                                        egui::RichText::new(&rule.pattern)
                                            .family(egui::FontFamily::Monospace)
                                            .color(theme.colors.on_surface_variant),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(
                                        egui::RichText::new(rule.priority.to_string())
                                            .color(theme.colors.on_surface_variant),
                                    );
                                });
                                row.col(|ui| {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .button(egui::RichText::new("🗑").size(14.0))
                                                .on_hover_text("Delete rule")
                                                .clicked()
                                            {
                                                to_delete = Some(idx);
                                            }
                                            ui.add_space(8.0);

                                            let is_editing_this = Some(idx) == dialog.editing_index;
                                            if ui
                                                .add_enabled(
                                                    !is_editing_this,
                                                    egui::Button::new(
                                                        egui::RichText::new("✏").size(14.0),
                                                    ),
                                                )
                                                .on_hover_text("Edit rule")
                                                .clicked()
                                            {
                                                to_edit = Some(idx);
                                            }
                                        },
                                    );
                                });
                            });
                        }
                    });
            });
    }

    // Apply actions after immutable borrow ends
    if let Some(idx) = to_delete {
        dialog.rules.remove(idx);
        // If we deleted the one we were editing, cancel edit
        if Some(idx) == dialog.editing_index {
            dialog.editing_index = None;
            dialog.edit_name.clear();
            dialog.edit_pattern.clear();
            dialog.edit_password.clear();
            dialog.edit_priority = "10".to_string();
            dialog.edit_enabled = true;
        } else if let Some(edit_idx) = dialog.editing_index {
            // Shift index if needed
            if idx < edit_idx {
                dialog.editing_index = Some(edit_idx - 1);
            }
        }
    }
    if let Some(idx) = to_edit {
        if let Some(rule) = dialog.rules.get(idx) {
            dialog.editing_index = Some(idx);
            dialog.edit_name = rule.name.clone();
            dialog.edit_pattern = rule.pattern.clone();
            dialog.edit_password = rule.password.clone();
            dialog.edit_priority = rule.priority.to_string();
            dialog.edit_enabled = rule.enabled;
        }
    }
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
