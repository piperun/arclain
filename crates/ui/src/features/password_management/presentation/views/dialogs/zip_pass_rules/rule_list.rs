use super::state::PasswordRulesDialog;
use crate::shared::theme::AppTheme;
use arclain_widgets::ToggleSwitch;
use eframe::egui;

pub fn render_rule_list(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
    content_width: f32,
) {
    // Rules list - Table format
    ui.label(
        egui::RichText::new("Saved password rules")
            .size(13.0)
            .color(theme.colors.on_surface_variant),
    );

    let mut to_delete: Option<usize> = None;
    let mut to_edit: Option<usize> = None;
    let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

    if dialog.rules.is_empty() {
        ui.label(
            egui::RichText::new("No password rules configured yet")
                .size(12.0)
                .color(theme.colors.on_surface_variant)
                .italics(),
        );
    } else {
        // Table header
        egui::Frame::NONE
            .fill(theme.colors.surface_variant)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_width(content_width - 20.0);
                    ui.label(egui::RichText::new("✓").size(12.0).strong());
                    ui.add_space(12.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(200.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new("Name").size(12.0).strong());
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(250.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new("Pattern").size(12.0).strong());
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(80.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new("Priority").size(12.0).strong());
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("Actions").size(12.0).strong());
                    });
                });
            });

        ui.add_space(4.0);

        // Table rows
        for (idx, rule) in dialog.rules.iter().enumerate() {
            let bg_color = if idx % 2 == 0 {
                theme.colors.surface_variant
            } else {
                theme.colors.surface
            };
            egui::Frame::NONE
                .fill(bg_color)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(egui::Stroke::new(0.5, theme.colors.outline))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_width(content_width - 20.0);
                        let mut enabled = rule.enabled;
                        if ui
                            .add(ToggleSwitch::new(&mut enabled).with_theme_colors(&theme.colors))
                            .changed()
                        {
                            enable_toggles.push((idx, enabled));
                        }
                        ui.add_space(12.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(200.0, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(egui::RichText::new(&rule.name).size(12.0).color(
                                    if rule.enabled {
                                        theme.colors.on_surface
                                    } else {
                                        theme.colors.on_surface_variant
                                    },
                                ));
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(250.0, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(&rule.pattern)
                                        .size(11.0)
                                        .family(egui::FontFamily::Monospace)
                                        .color(theme.colors.on_surface_variant),
                                );
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(80.0, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(rule.priority.to_string())
                                        .size(12.0)
                                        .color(theme.colors.on_surface_variant),
                                );
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(egui::RichText::new("🗑").size(14.0))
                                .on_hover_text("Delete rule")
                                .clicked()
                            {
                                to_delete = Some(idx);
                            }
                            ui.add_space(4.0);
                            if ui
                                .button(egui::RichText::new("✏").size(14.0))
                                .on_hover_text("Edit rule")
                                .clicked()
                            {
                                to_edit = Some(idx);
                            }
                        });
                    });
                });
        }
    }

    // Apply actions after immutable borrow ends
    if let Some(idx) = to_delete {
        dialog.rules.remove(idx);
    }
    if let Some(idx) = to_edit {
        if let Some(rule) = dialog.rules.get(idx) {
            dialog.editing_index = Some(idx);
            dialog.edit_name = rule.name.clone();
            dialog.edit_pattern = rule.pattern.clone();
            dialog.edit_password = rule.replacement_password.clone();
            dialog.edit_priority = rule.priority.to_string();
            dialog.edit_enabled = rule.enabled;
        }
    }
    for (idx, enabled) in enable_toggles {
        if let Some(r) = dialog.rules.get_mut(idx) {
            r.enabled = enabled;
        }
    }
}
