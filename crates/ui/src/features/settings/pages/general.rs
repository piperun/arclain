//! General Settings Page
//!
//! Contains the general settings page with appearance and behavior options.

use crate::features::settings::types::{GeneralSettingsState, SettingsAction};
use crate::shared::theme::AppTheme;
use eframe::egui;

use super::render_settings_section;

/// Render the General settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut GeneralSettingsState,
) -> Option<SettingsAction> {
    // Generic settings page

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Section: Appearance
        render_settings_section(ui, theme, "Appearance", |ui| {
            ui.label(
                egui::RichText::new("Theme settings and visual preferences")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            ui.label("Coming soon: Theme customization options");
        });

        ui.add_space(8.0);

        // Section: Behavior
        render_settings_section(ui, theme, "Behavior", |ui| {
            ui.label(
                egui::RichText::new("Application behavior and default actions")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(12.0);

            // Nested archive behavior
            ui.checkbox(
                &mut *state.open_nested_in_new_tab.write(),
                "Open nested archives in new tab",
            );
            ui.label(
                egui::RichText::new(if *state.open_nested_in_new_tab.read() {
                    "Nested archives will open in a new tab, preserving the current view"
                } else {
                    "Nested archives will replace the current archive view"
                })
                .size(11.0)
                .italics()
                .color(theme.colors.on_surface_variant),
            );
        });
    });

    None
}
