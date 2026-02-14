//! Network Settings Page
//!
//! Contains network and proxy configuration.

use arclain_theme::ThemeColors;
use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, TextButton, TextInput, TextInputSize, ToggleSwitch};
use crate::features::settings::types::{
    ConnectionTestResult, ConnectionTestStatus, NetworkSettingsState, SettingsAction,
};
use crate::shared::components::settings_form::{Form, SettingsGroup, SettingsRow};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render the Network settings page
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut NetworkSettingsState,
) -> Option<SettingsAction> {
    let mut action = None;

    Form::new().show(ui, theme, |ui| {
        // SOCKS5 Proxy Group
        SettingsGroup::new("SOCKS5 Proxy")
            .content(|ui, colors| {
                ui.label(
                    egui::RichText::new("Configure SOCKS5 proxy to bypass geo-blocking")
                        .size(12.0)
                        .color(colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Enable Proxy using SettingsRow with ToggleSwitch
                SettingsRow::new("Enable SOCKS5 Proxy")
                    .description("Route network requests through a SOCKS5 proxy server")
                    .action(|ui| {
                        ui.add(
                            ToggleSwitch::new(&mut *state.socks5_enabled.write())
                                .with_theme_colors(colors),
                        );
                    })
                    .show(ui, colors);

                ui.add_space(8.0);

                // Enabled Section
                ui.add_enabled_ui(*state.socks5_enabled.read(), |ui| {
                    ui.vertical(|ui| {
                        // Address
                        ui.label(
                            egui::RichText::new("Proxy Address")
                                .size(12.0)
                                .color(colors.on_surface),
                        );
                        TextInput::new(&mut *state.socks5_address.write())
                            .hint("e.g. 127.0.0.1:1080")
                            .size(TextInputSize::Small)
                            .width(ui.available_width())
                            .with_theme_colors(colors)
                            .show(ui);
                        ui.label(
                            egui::RichText::new("Hostname or IP address with port")
                                .size(11.0)
                                .color(colors.on_surface_variant),
                        );

                        ui.add_space(8.0);

                        // Authentication Grid
                        egui::Grid::new("socks5_auth_grid")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("Username")
                                        .size(12.0)
                                        .color(colors.on_surface),
                                );
                                TextInput::new(&mut *state.socks5_username.write())
                                    .size(TextInputSize::Small)
                                    .with_theme_colors(colors)
                                    .show(ui);
                                ui.end_row();

                                ui.label(
                                    egui::RichText::new("Password")
                                        .size(12.0)
                                        .color(colors.on_surface),
                                );
                                TextInput::new(&mut *state.socks5_password.write())
                                    .password(true)
                                    .size(TextInputSize::Small)
                                    .with_theme_colors(colors)
                                    .show(ui);
                                ui.end_row();
                            });

                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Leave blank if authentication is not required")
                                .size(11.0)
                                .color(colors.on_surface_variant),
                        );
                    });
                });

                ui.add_space(16.0);

                // Test Connection Section
                let status = state.connection_test_status.read().clone();

                // Test button
                if ui
                    .add(
                        TextButton::new("Test Connection", ButtonSize::Medium)
                            .with_theme_colors(colors),
                    )
                    .clicked()
                {
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

                // Results panel
                render_test_results_panel(ui, colors, &status);
            })
            .show(ui, &theme.colors);
    });

    action
}

/// Render the connection test results panel
fn render_test_results_panel(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    status: &ConnectionTestStatus,
) {
    match status {
        ConnectionTestStatus::Idle => {
            // Show nothing when idle
        }
        ConnectionTestStatus::Testing => {
            // Show testing indicator
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
        ConnectionTestStatus::Complete(result) => {
            render_test_result(ui, colors, result);
        }
    }
}

/// Render a complete test result with all steps
fn render_test_result(ui: &mut egui::Ui, colors: &ThemeColors, result: &ConnectionTestResult) {
    let bg_color = if result.success {
        colors.success.gamma_multiply(0.1)
    } else {
        colors.error.gamma_multiply(0.1)
    };

    let border_color = if result.success {
        colors.success.gamma_multiply(0.3)
    } else {
        colors.error.gamma_multiply(0.3)
    };

    egui::Frame::new()
        .fill(bg_color)
        .stroke(egui::Stroke::new(1.0, border_color))
        .corner_radius(6.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Header row with title and copy button
            ui.horizontal(|ui| {
                let (icon, title, title_color) = if result.success {
                    (
                        egui_phosphor::regular::CHECK_CIRCLE,
                        "Connection Successful",
                        colors.success,
                    )
                } else {
                    (
                        egui_phosphor::regular::X_CIRCLE,
                        "Connection Failed",
                        colors.error,
                    )
                };

                ui.label(egui::RichText::new(icon).size(16.0).color(title_color));
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .strong()
                        .color(title_color),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            IconButton::new(egui_phosphor::regular::COPY)
                                .size(IconButtonSize::Small)
                                .with_theme_colors(colors),
                        )
                        .on_hover_text("Copy to clipboard")
                        .clicked()
                    {
                        let text = format_result_for_clipboard(result);
                        ui.ctx().copy_text(text);
                    }
                });
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Test steps
            for step in &result.steps {
                render_test_step(ui, colors, step);
                ui.add_space(4.0);
            }

            // Success message
            if let Some(msg) = &result.result_message {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(egui_phosphor::regular::GLOBE)
                            .size(14.0)
                            .color(colors.on_surface_variant),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Connected via {}", msg))
                            .size(12.0)
                            .color(colors.on_surface),
                    );
                });
            }
        });
}

/// Render a single test step row
fn render_test_step(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    step: &crate::features::settings::types::TestStepResult,
) {
    ui.horizontal(|ui| {
        // Status icon
        let (icon, icon_color) = if step.passed {
            (egui_phosphor::regular::CHECK, colors.success)
        } else {
            (egui_phosphor::regular::X, colors.error)
        };

        ui.label(egui::RichText::new(icon).size(14.0).color(icon_color));
        ui.add_space(4.0);

        // Step name
        ui.label(
            egui::RichText::new(format!("{} Test", step.name))
                .size(12.0)
                .color(colors.on_surface),
        );

        // Status text
        let status_text = if step.passed { "passed" } else { "failed" };
        let status_color = if step.passed {
            colors.success
        } else {
            colors.error
        };
        ui.label(
            egui::RichText::new(format!("— {}", status_text))
                .size(12.0)
                .color(status_color),
        );
    });

    // Step message (success info or error details)
    if let Some(msg) = &step.message {
        let msg_color = if step.passed {
            colors.success.gamma_multiply(0.9)
        } else {
            colors.error.gamma_multiply(0.9)
        };
        ui.horizontal(|ui| {
            ui.add_space(22.0); // Indent to align with text
            ui.label(egui::RichText::new(msg).size(11.0).color(msg_color));
        });
    }
}

/// Format the test result for clipboard
fn format_result_for_clipboard(result: &ConnectionTestResult) -> String {
    let mut lines = Vec::new();

    lines.push(if result.success {
        "Connection Test: SUCCESS".to_string()
    } else {
        "Connection Test: FAILED".to_string()
    });

    lines.push(String::new());

    for step in &result.steps {
        let status = if step.passed { "✓" } else { "✗" };
        lines.push(format!("{} {} Test — {}", status, step.name, if step.passed { "passed" } else { "failed" }));

        if let Some(msg) = &step.message {
            lines.push(format!("  {}", msg));
        }
    }

    if let Some(msg) = &result.result_message {
        lines.push(String::new());
        lines.push(format!("Connected via {}", msg));
    }

    lines.join("\n")
}
