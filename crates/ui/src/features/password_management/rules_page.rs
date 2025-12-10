// Full-page view for password rules management (non-modal)
use crate::features::password_management::dialogs::zip_pass_rules::{
    PasswordRule, PasswordRulesDialog,
};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Result from rendering the password rules page
pub enum PasswordRulesPageResult {
    /// User clicked save
    Save,
}

/// Render the password rules management page (full-page, non-modal version)
/// Returns Some(PasswordRulesPageResult::Save) if the save button was clicked
pub fn render_password_rules_page(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
) -> Option<PasswordRulesPageResult> {
    let mut result = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            // Title section
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Password Rules Management")
                        .size(18.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
            });

            ui.label(
                egui::RichText::new(
                    "Configure automatic password matching for encrypted archives based on filename patterns",
                )
                .size(12.0)
                .color(theme.colors.text_secondary),
            );

            ui.add_space(16.0);

            // Rules list
            ui.label(
                egui::RichText::new("Saved password rules")
                    .size(14.0)
                    .strong()
                    .color(theme.colors.text_primary),
            );

            ui.add_space(8.0);

            let mut to_delete: Option<usize> = None;
            let mut to_edit: Option<usize> = None;
            let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

            if dialog.rules.is_empty() {
                egui::Frame::NONE
                    .fill(theme.colors.bg_secondary)
                    .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                    .corner_radius(8.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "No password rules configured yet. Add your first rule below.",
                            )
                            .size(13.0)
                            .color(theme.colors.text_secondary)
                            .italics(),
                        );
                    });
            } else {
                // Table container
                egui::Frame::NONE
                    .fill(theme.colors.bg_secondary)
                    .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                    .corner_radius(8.0)
                    .show(ui, |ui| {
                        // Table header
                        egui::Frame::NONE
                            .fill(theme.colors.bg_tertiary)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new("✓")
                                            .size(12.0)
                                            .strong(),
                                    );
                                    ui.add_space(12.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(200.0, 20.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("Name")
                                                    .size(12.0)
                                                    .strong(),
                                            );
                                        },
                                    );
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(250.0, 20.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("Pattern")
                                                    .size(12.0)
                                                    .strong(),
                                            );
                                        },
                                    );
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(80.0, 20.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("Priority")
                                                    .size(12.0)
                                                    .strong(),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("Actions")
                                                    .size(12.0)
                                                    .strong(),
                                            );
                                        },
                                    );
                                });
                            });

                        // Table rows
                        for (idx, rule) in dialog.rules.iter().enumerate() {
                            let bg_color = if idx % 2 == 0 {
                                theme.colors.bg_primary
                            } else {
                                theme.colors.bg_secondary
                            };
                            egui::Frame::NONE
                                .fill(bg_color)
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .stroke(egui::Stroke::new(0.5, theme.colors.border_color))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(ui.available_width());
                                        let mut enabled = rule.enabled;
                                        if ui.checkbox(&mut enabled, "").changed() {
                                            enable_toggles.push((idx, enabled));
                                        }
                                        ui.add_space(12.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(200.0, 20.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(&rule.name)
                                                        .size(12.0)
                                                        .color(if rule.enabled {
                                                            theme.colors.text_primary
                                                        } else {
                                                            theme.colors.text_secondary
                                                        }),
                                                );
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
                                                        .color(theme.colors.text_secondary),
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
                                                        .color(theme.colors.text_secondary),
                                                );
                                            },
                                        );
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
                                                ui.add_space(4.0);
                                                if ui
                                                    .button(egui::RichText::new("✏").size(14.0))
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
                    dialog.edit_password = rule.password.clone();
                    dialog.edit_priority = rule.priority.to_string();
                    dialog.edit_enabled = rule.enabled;
                }
            }
            for (idx, enabled) in enable_toggles {
                if let Some(r) = dialog.rules.get_mut(idx) {
                    r.enabled = enabled;
                }
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            // Edit form
            egui::Frame::NONE
                .fill(theme.colors.bg_secondary)
                .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                .corner_radius(8.0)
                .inner_margin(20.0)
                .show(ui, |ui| {
                    let form_title = if dialog.editing_index.is_some() {
                        "Edit password rule"
                    } else {
                        "Add new password rule"
                    };
                    ui.label(
                        egui::RichText::new(form_title)
                            .size(15.0)
                            .strong()
                            .color(theme.colors.text_primary),
                    );

                    ui.add_space(12.0);

                    egui::Grid::new("password_rule_form_page")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Name:")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary),
                            );
                            ui.add_sized(
                                [ui.available_width(), 28.0],
                                egui::TextEdit::singleline(&mut dialog.edit_name)
                                    .hint_text("e.g., Work archives"),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Pattern:")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary),
                            );
                            ui.horizontal(|ui| {
                                // Ensure minimum positive width for text edit
                                let pattern_width = (ui.available_width() - 130.0).max(100.0);
                                ui.add_sized(
                                    [pattern_width, 28.0],
                                    egui::TextEdit::singleline(&mut dialog.edit_pattern)
                                        .hint_text("e.g., work/*.7z")
                                        .font(egui::TextStyle::Monospace),
                                );
                                if ui.button("🧪 Test Regex").clicked() {
                                    dialog.show_regex_tester = true;
                                    dialog.regex_test_pattern = dialog.edit_pattern.clone();
                                    dialog.regex_test_results.clear();
                                }
                            });
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Password:")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary),
                            );
                            ui.add_sized(
                                [ui.available_width(), 28.0],
                                egui::TextEdit::singleline(&mut dialog.edit_password)
                                    .password(true)
                                    .hint_text("Archive password"),
                            );
                            ui.end_row();

                            ui.label(
                                egui::RichText::new("Priority:")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary),
                            );
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [80.0, 28.0],
                                    egui::TextEdit::singleline(&mut dialog.edit_priority)
                                        .hint_text("10"),
                                );
                                ui.add_space(20.0);
                                ui.checkbox(&mut dialog.edit_enabled, "Enabled");
                            });
                            ui.end_row();
                        });

                    ui.add_space(12.0);

                    ui.horizontal(|ui| {
                        let can_save = !dialog.edit_pattern.trim().is_empty()
                            && !dialog.edit_password.is_empty();
                        if ui
                            .add_enabled(
                                can_save,
                                egui::Button::new(
                                    egui::RichText::new(if dialog.editing_index.is_some() {
                                        "Update Rule"
                                    } else {
                                        "Add Rule"
                                    })
                                    .strong(),
                                )
                                .min_size(egui::vec2(120.0, 36.0)),
                            )
                            .clicked()
                        {
                            let priority = dialog.edit_priority.parse::<u32>().unwrap_or(10);
                            let new_rule = PasswordRule {
                                name: if dialog.edit_name.trim().is_empty() {
                                    dialog.edit_pattern.clone()
                                } else {
                                    dialog.edit_name.clone()
                                },
                                pattern: dialog.edit_pattern.clone(),
                                password: dialog.edit_password.clone(),
                                priority,
                                enabled: dialog.edit_enabled,
                            };
                            if let Some(idx) = dialog.editing_index {
                                dialog.rules[idx] = new_rule;
                                dialog.editing_index = None;
                            } else {
                                dialog.rules.push(new_rule);
                            }
                            dialog.edit_name.clear();
                            dialog.edit_pattern.clear();
                            dialog.edit_password.clear();
                            dialog.edit_priority = "10".to_string();
                            dialog.edit_enabled = true;
                        }

                        if dialog.editing_index.is_some()
                            && ui
                                .add(
                                    egui::Button::new("Cancel Edit")
                                        .min_size(egui::vec2(120.0, 36.0)),
                                )
                                .clicked()
                            {
                                dialog.editing_index = None;
                                dialog.edit_name.clear();
                                dialog.edit_pattern.clear();
                                dialog.edit_password.clear();
                                dialog.edit_priority = "10".to_string();
                                dialog.edit_enabled = true;
                            }
                    });

                    if !dialog.error.is_empty() {
                        ui.add_space(8.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 53, 69),
                            &dialog.error,
                        );
                    }

                    ui.add_space(20.0);

                    // Save button at the bottom
                    ui.horizontal(|ui| {
                        let save_btn = egui::Button::new(
                            egui::RichText::new("💾 Save All Changes")
                                .strong()
                        )
                        .min_size(egui::vec2(180.0, 40.0));

                        if ui.add(save_btn).clicked() {
                            result = Some(PasswordRulesPageResult::Save);
                        }

                        ui.label(
                            egui::RichText::new("Changes will be saved to the encrypted database")
                                .size(12.0)
                                .color(theme.colors.text_secondary)
                        );
                    });
                });
        });

    // Render regex tester modal if shown (this overlays the page)
    if dialog.show_regex_tester {
        super::dialogs::zip_pass_rules::tester::render_regex_tester_modal(ui.ctx(), theme, dialog);
    }

    result
}
