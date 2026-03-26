//! Server Settings Page
//!
//! Contains gameta server connection configuration.

use arclain_theme::ThemeColors;
use arclain_widgets::{ButtonSize, TextButton, TextInput, TextInputSize, ToggleSwitch};
use crate::features::settings::types::{
    ServerConnectionStatus, ServerSettingsState, SettingsAction,
};
use crate::shared::components::settings_form::{Form, SettingsGroup, SettingsRow};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the Server settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ServerSettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    Form::new().show(ui, theme, |ui| {
        SettingsGroup::new("Gameta Server")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new(
                        "Connect to a gameta server to fetch metadata for your archives",
                    )
                    .size(12.0)
                    .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Enable toggle
                SettingsRow::new("Enable Server")
                    .description("Fetch metadata from a gameta server instance")
                    .action(|ui| {
                        ui.add(
                            ToggleSwitch::new(&mut *state.enabled.write())
                                .with_theme_colors(colors),
                        );
                    })
                    .show(ui, colors);

                ui.add_space(8.0);

                ui.add_enabled_ui(*state.enabled.read(), |ui| {
                    ui.vertical(|ui| {
                        // Server URL
                        ui.label(
                            egui::RichText::new("Server URL")
                                .size(12.0)
                                .color(colors.on_surface),
                        );
                        TextInput::new(&mut *state.url.write())
                            .hint("e.g. http://localhost:8080")
                            .size(TextInputSize::Small)
                            .width(ui.available_width())
                            .with_theme_colors(colors)
                            .show(ui);
                        ui.label(
                            egui::RichText::new("Base URL of the gameta server")
                                .size(11.0)
                                .color(colors.on_surface_variant),
                        );

                        ui.add_space(8.0);

                        // API Key
                        ui.label(
                            egui::RichText::new("API Key")
                                .size(12.0)
                                .color(colors.on_surface),
                        );
                        TextInput::new(&mut *state.api_key.write())
                            .password(true)
                            .hint("Leave blank if authentication is not required")
                            .size(TextInputSize::Small)
                            .width(ui.available_width())
                            .with_theme_colors(colors)
                            .show(ui);
                        ui.label(
                            egui::RichText::new(
                                "Sent as a Bearer token — leave blank for unauthenticated access",
                            )
                            .size(11.0)
                            .color(colors.on_surface_variant),
                        );
                    });
                });

                ui.add_space(16.0);

                // Test Connection button
                let enabled = *state.enabled.read();
                let url_trimmed = state.url.read().trim().to_string();

                if ui
                    .add_enabled(
                        enabled && !url_trimmed.is_empty(),
                        TextButton::new("Test Connection", ButtonSize::Medium)
                            .with_theme_colors(colors),
                    )
                    .clicked()
                {
                    let api_key_opt = {
                        let k = state.api_key.read();
                        if k.trim().is_empty() {
                            None
                        } else {
                            Some(k.clone())
                        }
                    };
                    action = Some(SettingsAction::TestServer {
                        url: url_trimmed,
                        api_key: api_key_opt,
                    });
                }

                ui.add_space(8.0);

                // Connection status panel
                let status = state.connection_status.read().clone();
                render_connection_status(ui, colors, &status);
            })
            .show(ui, &theme.colors);
    });

    action
}

/// Render the connection status indicator
fn render_connection_status(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    status: &ServerConnectionStatus,
) {
    match status {
        ServerConnectionStatus::Idle => {
            // Show nothing when idle
        }
        ServerConnectionStatus::Testing => {
            egui::Frame::new()
                .fill(colors.surface_variant)
                .corner_radius(6.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Testing connection...")
                                .color(colors.on_surface_variant),
                        );
                    });
                });
        }
        ServerConnectionStatus::Connected(msg) => {
            let bg_color = colors.success.gamma_multiply(0.1);
            let border_color = colors.success.gamma_multiply(0.3);

            egui::Frame::new()
                .fill(bg_color)
                .stroke(egui::Stroke::new(1.0, border_color))
                .corner_radius(6.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                                .size(16.0)
                                .color(colors.success),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("Connected — {}", msg))
                                .size(13.0)
                                .strong()
                                .color(colors.success),
                        );
                    });
                });
        }
        ServerConnectionStatus::Failed(err) => {
            let bg_color = colors.error.gamma_multiply(0.1);
            let border_color = colors.error.gamma_multiply(0.3);

            egui::Frame::new()
                .fill(bg_color)
                .stroke(egui::Stroke::new(1.0, border_color))
                .corner_radius(6.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::X_CIRCLE)
                                .size(16.0)
                                .color(colors.error),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("Connection failed — {}", err))
                                .size(13.0)
                                .strong()
                                .color(colors.error),
                        );
                    });
                });
        }
    }
}
