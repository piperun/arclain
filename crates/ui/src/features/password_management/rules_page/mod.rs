use crate::features::password_management::dialogs::zip_pass_rules::PasswordRulesDialog;
use crate::shared::theme::AppTheme;
use eframe::egui;

mod form;
mod list;

/// Render the password rules management page (full-page, non-modal version)
pub fn render_password_rules_page(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
) {
    // Wrap page in a frame for margin/padding
    egui::Frame::NONE.inner_margin(24.0).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            // Section 1: Manage Rule (Form) - Moved to TOP
            form::render_form(ui, theme, dialog);

            ui.add_space(24.0);

            // Section 2: Rule Registry (List)
            list::render_list(ui, theme, dialog);
        });
    });

    // Render regex tester modal if shown (this overlays the page)
    if dialog.show_regex_tester {
        crate::features::password_management::dialogs::zip_pass_rules::tester::render_regex_tester_modal(ui.ctx(), theme, dialog);
    }
}
