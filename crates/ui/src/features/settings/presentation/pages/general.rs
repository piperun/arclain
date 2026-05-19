//! General Settings Page
//!
//! Contains the general settings page with appearance and behavior options.

use arclain_widgets::{ThemedDropdown, ToggleSwitch};
use crate::features::settings::types::{GeneralSettingsState, SettingsAction};
use crate::shared::components::settings_form::{Form, SettingsGroup, SettingsRow};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the General settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut GeneralSettingsState,
) -> Option<SettingsAction> {
    Form::new().show(ui, theme, |ui| {
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

                ui.add_space(8.0);

                // Drop behavior selector
                ui.label(
                    egui::RichText::new("When dropping an archive:")
                        .size(12.0)
                        .color(colors.on_surface),
                );
                ui.add_space(4.0);

                let current = *state.drop_behavior.read();
                let mut next = current;
                ThemedDropdown::new("settings_drop_behavior", current.display_name())
                    .with_theme_colors(colors)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for opt in [
                            arclain_core::DropBehavior::NewTab,
                            arclain_core::DropBehavior::Replace,
                            arclain_core::DropBehavior::AskEachTime,
                        ] {
                            if ui
                                .selectable_label(current == opt, opt.display_name())
                                .clicked()
                            {
                                next = opt;
                            }
                        }
                    });
                if next != current {
                    *state.drop_behavior.write() = next;
                }

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Applies when a file is dropped without aiming at a specific zone. \
                         Ctrl+drop always replaces the active tab.",
                    )
                    .size(11.0)
                    .italics()
                    .color(colors.on_surface_variant),
                );

                ui.add_space(12.0);

                // Restore tabs on launch toggle
                {
                    let mut restore = *state.restore_tabs_on_launch.read();
                    if ui.checkbox(&mut restore, "Restore tabs on launch").changed() {
                        *state.restore_tabs_on_launch.write() = restore;
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Re-open the previous session's tabs when the app starts.",
                    )
                    .size(11.0)
                    .italics()
                    .color(colors.on_surface_variant),
                );
            })
            .show(ui, &theme.colors);
    });

    None
}
