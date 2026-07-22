use super::state::PasswordRulesDialog;
use super::types::PasswordRule;
use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton, TextInput, TextInputSize};
use eframe::egui;

pub fn render_rule_editor(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
    content_width: f32,
) {
    // Edit form - improved layout
    let form_title = if dialog.editing_index.is_some() {
        "Edit password rule"
    } else {
        "Add new password rule"
    };
    ui.label(
        egui::RichText::new(form_title)
            .size(13.0)
            .strong()
            .color(theme.colors.on_surface),
    );

    ui.add_space(4.0);

    egui::Grid::new("password_rule_form")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("Name:")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            TextInput::new(&mut dialog.edit_name)
                .hint("e.g., Work archives")
                .size(TextInputSize::Small)
                .width(content_width - 120.0)
                .with_theme_colors(&theme.colors)
                .show(ui);
            ui.end_row();

            ui.label(
                egui::RichText::new("Pattern:")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.horizontal(|ui| {
                TextInput::new(&mut dialog.edit_pattern)
                    .hint("e.g., work/*.7z")
                    .monospace()
                    .size(TextInputSize::Small)
                    .width(content_width - 240.0)
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
            TextInput::new(&mut dialog.edit_password)
                .password(true)
                .hint("Archive password")
                .size(TextInputSize::Small)
                .width(content_width - 120.0)
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
                ui.checkbox(&mut dialog.edit_enabled, "Enabled");
            });
            ui.end_row();
        });

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let can_save = !dialog.edit_pattern.trim().is_empty() && !dialog.edit_password.is_empty();
        if ui
            .add_enabled(
                can_save,
                TextButton::new(
                    if dialog.editing_index.is_some() {
                        "Update Rule"
                    } else {
                        "Add Rule"
                    },
                    ButtonSize::Large,
                )
                .with_theme_colors(&theme.colors),
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
                    TextButton::new("Cancel Edit", ButtonSize::Medium)
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
        crate::shared::components::error_label(ui, theme, &dialog.error);
    }
}
