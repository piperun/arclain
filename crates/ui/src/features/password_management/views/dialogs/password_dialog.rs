use crate::shared::theme::AppTheme;
use eframe::egui;

// ================= Password Dialog =================

#[derive(Clone, Debug, Default)]
pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub save_password: bool,
    pub error: String,
    pub target_path: Option<std::path::PathBuf>,
}


pub enum PasswordDialogResult {
    Unlock,
    Cancel,
}

pub fn render_password_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordDialog,
) -> Option<PasswordDialogResult> {
    if !dialog.show {
        return None;
    }
    let mut result = None;

    // Dim overlay on a lower layer so it never covers the dialog
    egui::Area::new(egui::Id::new("password_overlay_dim"))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let screen = ctx.viewport_rect();
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
            // Visual-only overlay; don't capture input so the modal receives scroll.
        });

    // Modal dialog on the foreground layer
    egui::Area::new(egui::Id::new("password_modal"))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let screen = ctx.viewport_rect();
            // Slightly larger modal to avoid button overflow
            let width = 520.0;
            let height = if dialog.error.is_empty() { 340.0 } else { 380.0 };
            let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
            let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

            ui.painter().rect_filled(rect, 8.0, theme.colors.surface);
            ui.painter().rect_stroke(rect, egui::CornerRadius::same(8), egui::Stroke::new(1.0, theme.colors.outline), egui::StrokeKind::Outside);

            // Ensure elements are clipped to the modal rectangle
            ui.set_clip_rect(rect);

            let content = rect.shrink(24.0);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content)
                    .layout(egui::Layout::top_down(egui::Align::LEFT))
            );
            
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🔒").size(24.0));
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Archive Password Required")
                            .size(16.0)
                            .strong()
                            .color(theme.colors.on_surface)
                    );
                });

                ui.label(
                    egui::RichText::new("This archive is password-protected. Please enter the password to continue.")
                        .size(14.0)
                        .color(theme.colors.on_surface_variant)
                );

                let password_response = ui.add_sized(
                    [content.width(), 40.0], 
                    egui::TextEdit::singleline(&mut dialog.password)
                        .password(true)
                        .hint_text("Enter password...")
                        .font(egui::TextStyle::Body)
                );
                
                password_response.request_focus();
                
                // Press Enter to unlock while the field is focused
                if password_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !dialog.password.is_empty() {
                    result = Some(PasswordDialogResult::Unlock);
                }
                
                // Optional: ESC cancels
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    result = Some(PasswordDialogResult::Cancel);
                }

                // Save password checkbox
                ui.checkbox(&mut dialog.save_password, "Save password for future use");

                if !dialog.error.is_empty() { 
                    ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error); 
                }
                
                ui.add_space(8.0);
                
                ui.horizontal(|ui| {
                    ui.add_space(content.width() - 212.0);
                    
                    let cancel_btn = egui::Button::new(
                        egui::RichText::new("Cancel")
                            .size(14.0)
                            .color(theme.colors.on_surface)
                    )
                    .fill(theme.colors.surface_variant)
                    .stroke(egui::Stroke::new(1.0, theme.colors.outline))
                    .corner_radius(4.0)
                    .min_size(egui::vec2(100.0, 36.0));
                    
                    if ui.add(cancel_btn).clicked() { 
                        result = Some(PasswordDialogResult::Cancel); 
                    }
                    
                    ui.add_space(12.0);
                    
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
                    .corner_radius(4.0)
                    .min_size(egui::vec2(100.0, 36.0));
                    
                    if ui.add_enabled(unlock_enabled, unlock_btn).clicked() { 
                        result = Some(PasswordDialogResult::Unlock); 
                    }
                });
            });
        });

    result
}
