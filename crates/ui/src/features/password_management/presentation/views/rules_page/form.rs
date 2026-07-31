use crate::features::password_management::dialogs::zip_pass_rules::{
    PasswordRule, PasswordRulesDialog,
};
use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton, TextInput, TextInputSize, ToggleSwitch};
use eframe::egui;

pub fn render_form(ui: &mut egui::Ui, theme: &AppTheme, dialog: &mut PasswordRulesDialog) {
    // ---------------------------------------------------------
    // Section 1: Manage Rule (Form) - Moved to TOP
    // ---------------------------------------------------------

    // Header
    ui.horizontal(|ui| {
        let form_title = if dialog.editing_index.is_some() {
            "Edit Rule"
        } else {
            "Add New Rule"
        };
        ui.label(
            egui::RichText::new(form_title)
                //.size(15.0)
                .strong()
                .color(theme.colors.on_surface),
        );
        if dialog.editing_index.is_some() {
            ui.label(
                egui::RichText::new("(Editing Mode)")
                    .size(12.0)
                    .color(theme.colors.primary),
            );
        }
    });
    ui.add_space(4.0);

    // Form Container
    egui::Frame::NONE
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .corner_radius(8.0)
        .inner_margin(16.0)
        .show(ui, |ui| {
            egui::Grid::new("password_rule_form_page")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Name:")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    TextInput::new(&mut dialog.edit_name)
                        .hint("e.g., Work archives")
                        .size(TextInputSize::Small)
                        .width(ui.available_width())
                        .with_theme_colors(&theme.colors)
                        .show(ui);
                    ui.end_row();

                    ui.label(
                        egui::RichText::new("Pattern:")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    ui.horizontal(|ui| {
                        // Ensure minimum positive width for text edit
                        let pattern_width = (ui.available_width() - 130.0).max(100.0);
                        TextInput::new(&mut dialog.edit_pattern)
                            .hint("e.g., work/*.7z")
                            .monospace()
                            .size(TextInputSize::Small)
                            .width(pattern_width)
                            .with_theme_colors(&theme.colors)
                            .show(ui);
                        if ui
                            .add(
                                TextButton::new(
                                    format!("{} Test Regex", egui_phosphor::regular::TEST_TUBE),
                                    ButtonSize::Medium,
                                )
                                .with_theme_colors(&theme.colors),
                            )
                            .clicked()
                        {
                            dialog.show_regex_tester = true;
                            dialog.regex_test_pattern = dialog.edit_pattern.clone();
                            dialog.regex_test_results.clear();
                        }
                    });
                    ui.end_row();

                    ui.label(
                        egui::RichText::new("Password:")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    let preserves_stored_password = dialog
                        .editing_index
                        .and_then(|index| dialog.rules.get(index))
                        .map(|rule| rule.original_name.is_some() && rule.password_configured)
                        .unwrap_or(false);
                    TextInput::new(&mut dialog.edit_password)
                        .password(true)
                        .hint(if preserves_stored_password {
                            "Leave blank to keep the saved password"
                        } else {
                            "Archive password"
                        })
                        .size(TextInputSize::Small)
                        .width(ui.available_width())
                        .with_theme_colors(&theme.colors)
                        .show(ui);
                    ui.end_row();

                    ui.label(
                        egui::RichText::new("Priority:")
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                    ui.horizontal(|ui| {
                        TextInput::new(&mut dialog.edit_priority)
                            .hint("10")
                            .size(TextInputSize::Small)
                            .width(80.0)
                            .with_theme_colors(&theme.colors)
                            .show(ui);
                        ui.add_space(20.0);
                        ui.add(
                            ToggleSwitch::new(&mut dialog.edit_enabled)
                                .with_theme_colors(&theme.colors),
                        );
                        ui.label("Enabled");
                    });
                    ui.end_row();
                });

            ui.add_space(16.0);

            // Action Buttons
            ui.horizontal(|ui| {
                let can_save = dialog.can_save_edit();

                let btn_text = if dialog.editing_index.is_some() {
                    "Update Rule"
                } else {
                    "Add Rule"
                };

                if ui
                    .add_enabled(
                        can_save,
                        arclain_widgets::button::TextButton::new(
                            btn_text,
                            arclain_widgets::button::ButtonSize::Large,
                        )
                        .variant(arclain_theme::ButtonVariant::Primary)
                        .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    let priority = dialog.edit_priority.parse::<u32>().unwrap_or(10);
                    let previous = dialog
                        .editing_index
                        .and_then(|index| dialog.rules.get(index));
                    let new_rule = PasswordRule {
                        original_name: previous.and_then(|rule| rule.original_name.clone()),
                        name: if dialog.edit_name.trim().is_empty() {
                            dialog.edit_pattern.clone()
                        } else {
                            dialog.edit_name.clone()
                        },
                        pattern: dialog.edit_pattern.clone(),
                        replacement_password: dialog.edit_password.clone(),
                        password_configured: previous
                            .map(|rule| rule.password_configured)
                            .unwrap_or(false)
                            || !dialog.edit_password.is_empty(),
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
                            TextButton::new("Cancel", ButtonSize::Small)
                                .with_theme_colors(&theme.colors),
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
                crate::shared::components::error_label(ui, theme, &dialog.error);
            }
        });
}
