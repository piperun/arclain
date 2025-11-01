use super::theme::AppTheme;
use eframe::egui;

// ================= Password Dialog =================

pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub error: String,
}

impl Default for PasswordDialog {
    fn default() -> Self { Self { show: false, password: String::new(), error: String::new() } }
}

pub enum PasswordDialogResult { Unlock, Cancel }

pub fn render_password_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordDialog,
) -> Option<PasswordDialogResult> {
    if !dialog.show { return None; }
    let mut result = None;

    // Dim overlay on a lower layer so it never covers the dialog
    egui::Area::new(egui::Id::new("password_overlay_dim")).order(egui::Order::Middle).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        let _ = ui.allocate_rect(screen, egui::Sense::click());
    });

    // Modal dialog on the foreground layer
    egui::Area::new(egui::Id::new("password_modal")).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        // Slightly larger modal to avoid button overflow
        let width = 520.0;
        let height = if dialog.error.is_empty() { 300.0 } else { 340.0 };
        let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
        let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

        ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));

        let content = rect.shrink(24.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::LEFT)));
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🔒").size(24.0));
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Archive Password Required").size(16.0).strong().color(theme.colors.text_primary));
            });

            ui.label(egui::RichText::new("This archive is password-protected. Please enter the password to continue.").size(14.0).color(theme.colors.text_secondary));

            let password_response = ui.add_sized([content.width(), 40.0], egui::TextEdit::singleline(&mut dialog.password).password(true).hint_text("Enter password...").font(egui::TextStyle::Body));
            password_response.request_focus();
            // Press Enter to unlock while the field is focused
            if password_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !dialog.password.is_empty() {
                result = Some(PasswordDialogResult::Unlock);
            }
            // Optional: ESC cancels
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                result = Some(PasswordDialogResult::Cancel);
            }

            if !dialog.error.is_empty() { ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(content.width() - 212.0);
                let cancel_btn = egui::Button::new(egui::RichText::new("Cancel").size(14.0).color(theme.colors.text_primary)).fill(theme.colors.bg_tertiary).stroke(egui::Stroke::new(1.0, theme.colors.border_color)).rounding(4.0).min_size(egui::vec2(100.0, 36.0));
                if ui.add(cancel_btn).clicked() { result = Some(PasswordDialogResult::Cancel); }
                ui.add_space(12.0);
                let unlock_enabled = !dialog.password.is_empty();
                let unlock_btn = egui::Button::new(egui::RichText::new("Unlock").size(14.0).strong().color(if theme.dark_mode { egui::Color32::BLACK } else { egui::Color32::WHITE })).fill(if theme.dark_mode { egui::Color32::WHITE } else { egui::Color32::BLACK }).rounding(4.0).min_size(egui::vec2(100.0, 36.0));
                if ui.add_enabled(unlock_enabled, unlock_btn).clicked() { result = Some(PasswordDialogResult::Unlock); }
            });
        });
    });

    result
}

// ================= File Edit Dialog =================

pub struct FileEditDialog {
    pub show: bool,
    pub full_path_in_archive: String,
    pub name_input: String,
    pub content: String,
    pub error: String,
}

impl Default for FileEditDialog {
    fn default() -> Self { Self { show: false, full_path_in_archive: String::new(), name_input: String::new(), content: String::new(), error: String::new() } }
}

pub enum FileEditResult { Save { new_name: String, content: String }, Cancel }

pub fn render_file_edit_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut FileEditDialog,
) -> Option<FileEditResult> {
    if !dialog.show { return None; }
    let mut result = None;

    egui::Area::new(egui::Id::new("file_edit_overlay_dim")).order(egui::Order::Middle).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
        let _ = ui.allocate_rect(screen, egui::Sense::click());
    });

    egui::Area::new(egui::Id::new("file_edit_modal")).order(egui::Order::Foreground).show(ctx, |ui| {
        let screen = ctx.screen_rect();
        let width = (screen.width() * 0.6).clamp(520.0, 900.0);
        let height = (screen.height() * 0.7).clamp(420.0, 900.0);
        let pos = egui::pos2((screen.width() - width) / 2.0, (screen.height() - height) / 2.0);
        let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

        ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, theme.colors.border_color));

        let content_rect = rect.shrink2(egui::vec2(20.0, 16.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content_rect).layout(egui::Layout::top_down_justified(egui::Align::Min)));
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("✏ Edit File").size(18.0).strong());
                ui.label(egui::RichText::new("— inline editor").size(12.0).color(theme.colors.text_secondary));
            });

            ui.label(egui::RichText::new("File name").size(12.0).color(theme.colors.text_secondary));
            ui.add_sized([content_rect.width(), 32.0], egui::TextEdit::singleline(&mut dialog.name_input));

            ui.label(egui::RichText::new("Content").size(12.0).color(theme.colors.text_secondary));
            ui.add_sized([content_rect.width(), content_rect.height() - 140.0], egui::TextEdit::multiline(&mut dialog.content).font(egui::TextStyle::Monospace).code_editor());

            if !dialog.error.is_empty() { ui.colored_label(egui::Color32::from_rgb(220, 53, 69), &dialog.error); }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = ui.add(egui::Button::new(egui::RichText::new("Save").strong()).min_size(egui::vec2(100.0, 32.0)));
                let cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0)));
                if save.clicked() { result = Some(FileEditResult::Save { new_name: dialog.name_input.clone(), content: dialog.content.clone() }); }
                if cancel.clicked() { result = Some(FileEditResult::Cancel); }
            });
        });
    });

    result
}

