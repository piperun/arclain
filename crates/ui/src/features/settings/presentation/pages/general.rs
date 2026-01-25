//! General Settings Page
//!
//! Contains the general settings page with appearance and behavior options.

use arclain_widgets::ToggleSwitch;
use crate::features::settings::types::{GeneralSettingsState, SettingsAction};
use crate::shared::components::settings_form::{SettingsForm, SettingsGroup, SettingsRow};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the General settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut GeneralSettingsState,
) -> Option<SettingsAction> {
    SettingsForm::new().show(ui, theme, |ui| {
        // Section: Appearance
        SettingsGroup::new("Appearance")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Theme settings and visual preferences")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(8.0);

                ui.label(
                    egui::RichText::new("Coming soon: Theme customization options")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
            })
            .show(ui, &theme.colors);

        // Section: Behavior
        SettingsGroup::new("Behavior")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Application behavior and default actions")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Nested archive behavior using SettingsRow with ToggleSwitch
                let description = if *state.open_nested_in_new_tab.read() {
                    "Nested archives will open in a new tab, preserving the current view"
                } else {
                    "Nested archives will replace the current archive view"
                };

                SettingsRow::new("Open nested archives in new tab")
                    .description(description)
                    .action(|ui| {
                        ui.add(
                            ToggleSwitch::new(&mut *state.open_nested_in_new_tab.write())
                                .with_theme_colors(colors),
                        );
                    })
                    .show(ui, colors);
            })
            .show(ui, &theme.colors);
    });

    None
}
