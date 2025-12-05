// UI rendering for Zip Password Rules dialog
use crate::shared::theme::AppTheme;
use eframe::egui;

use super::state::PasswordRulesDialog;
use super::types::PasswordRule;

/// Result of the password rules dialog
pub enum PasswordRulesResult {
    Save { rules: Vec<PasswordRule> },
    Cancel,
}

/// Render the password rules management dialog
pub fn render_password_rules_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
) -> Option<PasswordRulesResult> {
    if !dialog.show {
        return None;
    }
    let mut result = None;

    // Dim overlay - capture all input to block background interaction
    egui::Area::new(egui::Id::new("pass_rules_overlay_dim"))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let screen = ctx.viewport_rect();
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
            // Sense all input on the overlay to block interaction with content behind it
            ui.allocate_rect(screen, egui::Sense::click_and_drag());
        });

    // Modal
    egui::Area::new(egui::Id::new("pass_rules_modal"))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let screen = ctx.viewport_rect();
            let width = (screen.width() * 0.75).clamp(800.0, 1200.0);
            let height = (screen.height() * 0.8).clamp(600.0, 900.0);
            let pos = egui::pos2(
                (screen.width() - width) / 2.0,
                (screen.height() - height) / 2.0,
            );
            let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

            ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, theme.colors.border_color),
                egui::StrokeKind::Outside,
            );

            // Clip all content to the modal rectangle so nothing spills out.
            ui.set_clip_rect(rect);

            // Ensure everything drawn for this modal is clipped to its rect so
            // large content (like text logs) cannot spill outside.
            ui.set_clip_rect(rect);

            let content = rect.shrink2(egui::vec2(20.0, 16.0));
            // Reserve space for a non-scrollable bottom bar
            let bottom_bar_h = 44.0;
            let scroll_rect = egui::Rect::from_min_max(
                content.min,
                egui::pos2(content.max.x, content.max.y - bottom_bar_h - 6.0),
            );
            let bottom_rect = egui::Rect::from_min_max(
                egui::pos2(content.min.x, content.max.y - bottom_bar_h),
                content.max,
            );

            // Main scrollable content area
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(scroll_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );

            child.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

                        // Title
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("🔐 Password Rules")
                                    .size(18.0)
                                    .strong()
                                    .color(theme.colors.text_primary),
                            );
                            ui.label(
                                egui::RichText::new("— manage encrypted archive passwords")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary),
                            );
                        });

                        ui.add_space(8.0);

                        // Rules list - Table format
                        ui.label(
                            egui::RichText::new("Saved password rules")
                                .size(13.0)
                                .color(theme.colors.text_secondary),
                        );

                        let mut to_delete: Option<usize> = None;
                        let mut to_edit: Option<usize> = None;
                        let mut enable_toggles: Vec<(usize, bool)> = Vec::new();

                        if dialog.rules.is_empty() {
                            ui.label(
                                egui::RichText::new("No password rules configured yet")
                                    .size(12.0)
                                    .color(theme.colors.text_secondary)
                                    .italics(),
                            );
                        } else {
                            // Table header
                            egui::Frame::NONE
                                .fill(theme.colors.bg_tertiary)
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(content.width() - 20.0);
                                        ui.label(egui::RichText::new("✓").size(12.0).strong());
                                        ui.add_space(12.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(200.0, 20.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new("Name").size(12.0).strong(),
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

                            ui.add_space(4.0);

                            // Table rows
                            for (idx, rule) in dialog.rules.iter().enumerate() {
                                let bg_color = if idx % 2 == 0 {
                                    theme.colors.bg_secondary
                                } else {
                                    theme.colors.bg_primary
                                };
                                egui::Frame::NONE
                                    .fill(bg_color)
                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                    .stroke(egui::Stroke::new(0.5, theme.colors.border_color))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.set_min_width(content.width() - 20.0);
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
                                                        egui::RichText::new(
                                                            rule.priority.to_string(),
                                                        )
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

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

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
                                .color(theme.colors.text_primary),
                        );

                        ui.add_space(4.0);

                        egui::Grid::new("password_rule_form")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("Name:")
                                        .size(12.0)
                                        .color(theme.colors.text_secondary),
                                );
                                ui.add_sized(
                                    [content.width() - 120.0, 28.0],
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
                                    ui.add_sized(
                                        [content.width() - 240.0, 28.0],
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
                                    [content.width() - 120.0, 28.0],
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

                        ui.add_space(8.0);

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
                                    .min_size(egui::vec2(100.0, 32.0)),
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
                                            .min_size(egui::vec2(100.0, 32.0)),
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
                            ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error);
                        }
                    });
            });

            // Fixed bottom action bar
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bottom_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            bar.horizontal(|ui| {
                let bar_rect = ui.max_rect();
                // subtle top separator
                let sep_rect = egui::Rect::from_min_max(
                    egui::pos2(bar_rect.min.x, bar_rect.min.y - 6.0),
                    egui::pos2(bar_rect.max.x, bar_rect.min.y - 5.0),
                );
                ui.painter()
                    .rect_filled(sep_rect, 0.0, theme.colors.border_color);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save_btn = egui::Button::new(egui::RichText::new("Save All").strong())
                        .min_size(egui::vec2(120.0, 36.0));
                    let cancel_btn = egui::Button::new("Cancel").min_size(egui::vec2(120.0, 36.0));

                    if ui.add(cancel_btn).clicked() {
                        result = Some(PasswordRulesResult::Cancel);
                    }
                    if ui.add(save_btn).clicked() {
                        result = Some(PasswordRulesResult::Save {
                            rules: dialog.rules.clone(),
                        });
                    }
                });
            });
        });

    // Render regex tester modal if shown
    if dialog.show_regex_tester {
        super::tester::render_regex_tester_modal(ctx, theme, dialog);
    }

    result
}
