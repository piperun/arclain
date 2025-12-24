//! Network Settings Page
//!
//! Contains network and proxy configuration.

use crate::features::settings::types::{NetworkSettingsState, SettingsAction};
use crate::shared::components::settings_form::{SettingsForm, SettingsGroup};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the Network settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut NetworkSettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    SettingsForm::new().show(ui, theme, |ui| {
        // SOCKS5 Proxy Group
        SettingsGroup::new("SOCKS5 Proxy")
            .content(|ui, _colors| {
                ui.label(
                    egui::RichText::new("Configure SOCKS5 proxy to bypass geo-blocking")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Enable Proxy
                if ui
                    .checkbox(&mut *state.socks5_enabled.write(), "Enable SOCKS5 Proxy")
                    .changed()
                {
                    // Trigger save on toggle? Or wait for save button?
                    // Settings pages usually wait for explicit save/nav or have auto-save.
                    // The standard here seems to be "dirty checks" and explicit save in the header.
                    // But we can also trigger update actions.
                    // For now, adhere to state mutation, let header save handle it.
                }

                ui.add_space(8.0);

                // Enabled Section
                ui.add_enabled_ui(*state.socks5_enabled.read(), |ui| {
                    ui.vertical(|ui| {
                        // Address
                        ui.label("Proxy Address");
                        ui.add(
                            egui::TextEdit::singleline(&mut *state.socks5_address.write())
                                .hint_text("e.g. 127.0.0.1:1080")
                                .desired_width(f32::INFINITY),
                        );
                        ui.label(
                            egui::RichText::new("Hostname or IP address with port")
                                .size(11.0)
                                .color(theme.colors.on_surface_variant),
                        );

                        ui.add_space(8.0);

                        // Authentication Grid
                        egui::Grid::new("socks5_auth_grid")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("Username");
                                ui.text_edit_singleline(&mut *state.socks5_username.write());
                                ui.end_row();

                                ui.label("Password");
                                ui.add(
                                    egui::TextEdit::singleline(&mut *state.socks5_password.write())
                                        .password(true),
                                );
                                ui.end_row();
                            });

                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Leave blank if authentication is not required")
                                .size(11.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                });

                ui.add_space(12.0);

                // Test Connection
                let status = state.connection_test_status.read().clone();
                ui.horizontal(|ui| {
                    if ui.button("Test Connection").clicked() {
                        let address_opt = if state.socks5_address.read().trim().is_empty() {
                            None
                        } else {
                            Some(state.socks5_address.read().trim().to_string())
                        };
                        let username_opt = if state.socks5_username.read().trim().is_empty() {
                            None
                        } else {
                            Some(state.socks5_username.read().trim().to_string())
                        };
                        // Pass current password state
                        let password_opt = if state.socks5_password.read().is_empty() {
                            None
                        } else {
                            Some(state.socks5_password.read().clone())
                        };

                        action = Some(SettingsAction::TestNetwork {
                            socks5_enabled: *state.socks5_enabled.read(),
                            socks5_address: address_opt,
                            socks5_username: username_opt,
                            socks5_password: password_opt,
                        });
                    }

                    ui.add_space(8.0);

                    use crate::features::settings::types::ConnectionTestStatus;
                    match status {
                        ConnectionTestStatus::Idle => {}
                        ConnectionTestStatus::Testing => {
                            ui.spinner();
                            ui.label("Connecting...");
                        }
                        ConnectionTestStatus::Success(msg) => {
                            ui.label(
                                egui::RichText::new(format!("✔ {}", msg))
                                    .color(theme.colors.success),
                            );
                        }
                        ConnectionTestStatus::Error(err) => {
                            ui.label(
                                egui::RichText::new(format!("✖ {}", err)).color(theme.colors.error),
                            );
                        }
                    }
                });
            })
            .show(ui, &theme.colors);
    });

    action
}
