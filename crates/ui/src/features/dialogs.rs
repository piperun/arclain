use super::theme::AppTheme;
use eframe::egui;

pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub error: String,
}

impl Default for PasswordDialog {
    fn default() -> Self {
        Self {
            show: false,
            password: String::new(),
            error: String::new(),
        }
    }
}

pub fn render_password_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordDialog,
) -> Option<PasswordDialogResult> {
    let mut result = None;

    if dialog.show {
        // Semi-transparent overlay
        egui::Area::new("password_overlay".into())
            .fixed_pos(egui::pos2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let screen_rect = ctx.screen_rect();

                // Dark overlay
                ui.painter().rect_filled(
                    screen_rect,
                    0.0,
                    egui::Color32::from_black_alpha(180),
                );

                // Dialog box
                let dialog_width = 420.0;
                let dialog_height = if dialog.error.is_empty() { 220.0 } else { 260.0 };
                let dialog_pos = egui::pos2(
                    (screen_rect.width() - dialog_width) / 2.0,
                    (screen_rect.height() - dialog_height) / 2.0,
                );

                let dialog_rect = egui::Rect::from_min_size(
                    dialog_pos,
                    egui::vec2(dialog_width, dialog_height),
                );

                // Dialog background
                ui.painter().rect_filled(
                    dialog_rect,
                    8.0,
                    theme.colors.bg_primary,
                );

                // Dialog border
                ui.painter().rect_stroke(
                    dialog_rect,
                    8.0,
                    egui::Stroke::new(1.0, theme.colors.border_color),
                );

                // Content
                let content_rect = dialog_rect.shrink(24.0);
                let mut child_ui = ui.new_child(egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)));

                child_ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

                    // Title with icon
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("🔒").size(24.0));
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Archive Password Required")
                                .size(16.0)
                                .strong()
                                .color(theme.colors.text_primary)
                        );
                    });

                    // Description
                    ui.label(
                        egui::RichText::new(
                            "This archive is password-protected. Please enter the password to continue."
                        )
                        .size(14.0)
                        .color(theme.colors.text_secondary)
                    );

                    // Password input
                    let password_response = ui.add_sized(
                        [content_rect.width(), 40.0],
                        egui::TextEdit::singleline(&mut dialog.password)
                            .password(true)
                            .hint_text("Enter password...")
                            .font(egui::TextStyle::Body)
                    );

                    password_response.request_focus();

                    // Handle Enter key
                    if password_response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && !dialog.password.is_empty()
                    {
                        result = Some(PasswordDialogResult::Unlock);
                    }

                    // Error message
                    if !dialog.error.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 53, 69),
                            &dialog.error
                        );
                    }

                    ui.add_space(8.0);

                    // Buttons
                    ui.horizontal(|ui| {
                        ui.add_space(content_rect.width() - 212.0);

                        // Cancel button
                        let cancel_btn = egui::Button::new(
                            egui::RichText::new("Cancel")
                                .size(14.0)
                                .color(theme.colors.text_primary)
                        )
                        .fill(theme.colors.bg_tertiary)
                        .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
                        .rounding(4.0)
                        .min_size(egui::vec2(100.0, 36.0));
                        if ui.add(cancel_btn).clicked() {
                            result = Some(PasswordDialogResult::Cancel);
                        }

                        ui.add_space(12.0);

                        // Unlock button
                        let unlock_enabled = !dialog.password.is_empty();
                        let unlock_btn = egui::Button::new(
                            egui::RichText::new("Unlock")
                                .size(14.0)
                                .strong()
                                .color(if theme.dark_mode {
                                    egui::Color32::BLACK
                                } else {
                                    egui::Color32::WHITE
                                })
                        )
                        .fill(if theme.dark_mode {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::BLACK
                        })
                        .rounding(4.0)
                        .min_size(egui::vec2(100.0, 36.0));
                        if ui.add_enabled(unlock_enabled, unlock_btn).clicked() {
                            result = Some(PasswordDialogResult::Unlock);
                        }
                    });
                });
            });
    }

    result
}

pub enum PasswordDialogResult {
    Unlock,
    Cancel,
}
