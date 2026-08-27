// UI rendering for Zip Password Rules dialog
use crate::shared::theme::AppTheme;
use arclain_theme::ButtonVariant;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

use super::state::PasswordRulesDialog;
use super::types::PasswordRule;
use super::{rule_editor, rule_list};

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

            ui.painter().rect_filled(rect, 8.0, theme.colors.surface);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0_f32, theme.colors.outline),
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
                                    .color(theme.colors.on_surface),
                            );
                            ui.label(
                                egui::RichText::new("— manage encrypted archive passwords")
                                    .size(12.0)
                                    .color(theme.colors.on_surface_variant),
                            );
                        });

                        ui.add_space(8.0);

                        // Rules list extracted to rule_list.rs
                        rule_list::render_rule_list(ui, theme, dialog, content.width());

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // Edit form extracted to rule_editor.rs
                        rule_editor::render_rule_editor(ui, theme, dialog, content.width());
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
                    .rect_filled(sep_rect, 0.0, theme.colors.outline);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let action_btn_size = ButtonSize::Custom {
                        width: 120.0,
                        height: 36.0,
                    };

                    if ui
                        .add(
                            TextButton::new("Cancel", action_btn_size)
                                .variant(ButtonVariant::Secondary)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        result = Some(PasswordRulesResult::Cancel);
                    }
                    if ui
                        .add(
                            TextButton::new("Save All", action_btn_size)
                                .variant(ButtonVariant::Primary)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
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
