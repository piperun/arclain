use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the Interface settings page
pub fn render_interface_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Section: Buttons
        render_section(ui, theme, "Buttons & Controls", |ui| {
            ui.label(
                egui::RichText::new("Configure how buttons and controls appear")
                    .size(12.0)
                    .color(theme.colors.text_secondary),
            );
            ui.add_space(8.0);

            // Show button labels toggle
            let mut show_labels = {
                let state = app_state.lock();
                state.ui_preferences.show_button_labels
            };

            if ui
                .checkbox(&mut show_labels, "Show button labels")
                .on_hover_text("Display text labels next to icons in header and toolbar buttons")
                .changed()
            {
                let mut state = app_state.lock();
                state.ui_preferences.show_button_labels = show_labels;
            }
        });

        ui.add_space(8.0);

        // Section: Theme (placeholder)
        render_section(ui, theme, "Theme", |ui| {
            ui.label(
                egui::RichText::new("Visual theme customization")
                    .size(12.0)
                    .color(theme.colors.text_secondary),
            );
            ui.add_space(8.0);

            ui.label("Coming soon: Custom theme colors and fonts");
        });
    });
}

/// Helper function to render a settings section with consistent styling
fn render_section<R>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(theme.colors.bg_secondary)
        .stroke(egui::Stroke::new(1.0, theme.colors.border_color))
        .corner_radius(8.0)
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );
                ui.add_space(8.0);
                content(ui)
            })
            .inner
        })
        .inner
}
