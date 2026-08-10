//! Permission review for a selected `.wirt` package.

use crate::features::plugins::domain::types::PendingPluginInstall;
use crate::shared::dialogs::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use arclain_app::error::ApplicationErrorKind;
use arclain_app::plugins::PluginCapabilityDto;
use arclain_widgets::{ButtonSize, Text, TextButton};
use eframe::egui;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginInstallDialogResult {
    Cancel,
    Install {
        package_path: PathBuf,
        expected_fingerprint: String,
    },
}

pub fn render(
    ctx: &egui::Context,
    theme: &AppTheme,
    pending: &mut PendingPluginInstall,
) -> Option<PluginInstallDialogResult> {
    let package_path = pending.package_path.clone();
    let preview = pending.preview.clone();
    let loading = pending.loading;
    let installing = pending.installing;
    let error_kind = pending.error_kind.clone();
    let error = pending.error.clone();
    let mut result = None;
    let params = ModalParams {
        width_frac: 0.52,
        height_frac: 0.62,
        min: egui::vec2(520.0, 420.0),
        max: egui::vec2(760.0, 760.0),
        bottom_bar_height: 52.0,
        ..Default::default()
    };

    show_dimmed_modal(
        ctx,
        theme,
        "plugin_install_review",
        &params,
        |ui, _| {
            Text::new("Review Wirt plugin").size(20.0).strong().show(ui);
            ui.add_space(4.0);
            Text::new("Approve only the permissions and destinations shown here.")
                .muted()
                .show(ui);
            ui.add_space(16.0);

            if let Some(preview) = preview.as_ref() {
                Text::new(&preview.name).size(18.0).strong().show(ui);
                Text::new(&preview.plugin_id).monospace().show(ui);
                Text::new(&format!("Version {}", preview.version)).show(ui);
                let author = Text::new(&preview.author).to_rich_text(ui);
                ui.add(egui::Label::new(author).wrap());
                Text::new(&format!("Wirt ABI {}", preview.abi)).show(ui);

                ui.add_space(18.0);
                Text::new("Requested permissions").strong().show(ui);
                ui.add_space(6.0);
                if preview.capabilities.is_empty() {
                    Text::new("No additional permissions requested")
                        .muted()
                        .show(ui);
                } else {
                    for capability in &preview.capabilities {
                        ui.horizontal(|ui| {
                            Text::new("•").muted().show(ui);
                            Text::new(capability.label()).show(ui);
                        });
                    }
                }

                if preview.capabilities.contains(&PluginCapabilityDto::Network) {
                    ui.add_space(14.0);
                    Text::new("Network domains").strong().show(ui);
                    ui.add_space(6.0);
                    for domain in &preview.network_domains {
                        Text::new(domain).monospace().show(ui);
                    }
                }

                ui.add_space(18.0);
                Text::new("Package fingerprint").strong().show(ui);
                ui.add_space(6.0);
                Text::new(&preview.fingerprint)
                    .size(12.0)
                    .monospace()
                    .show(ui);
            } else if loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    Text::new("Inspecting Wirt package…").show(ui);
                });
            }

            if let Some(error) = error.as_deref() {
                ui.add_space(16.0);
                Text::new(package_failure_heading(error_kind.as_ref()))
                    .strong()
                    .color(theme.colors.error)
                    .show(ui);
                Text::new(error).color(theme.colors.error).show(ui);
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(preview) = preview.as_ref() {
                    let install_label = if installing {
                        "Installing…"
                    } else {
                        "Install"
                    };
                    if ui
                        .add_enabled(
                            !loading,
                            TextButton::new(install_label, ButtonSize::Medium)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        result = Some(PluginInstallDialogResult::Install {
                            package_path: package_path.clone(),
                            expected_fingerprint: preview.fingerprint.clone(),
                        });
                    }
                }
                if ui
                    .add_enabled(
                        !installing,
                        TextButton::new("Cancel", ButtonSize::Medium)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    result = Some(PluginInstallDialogResult::Cancel);
                }
            });
        },
    );

    result
}

fn package_failure_heading(kind: Option<&ApplicationErrorKind>) -> &'static str {
    match kind {
        Some(ApplicationErrorKind::InvalidInput) => "Invalid Wirt package",
        Some(ApplicationErrorKind::Unsupported) => "Unsupported Wirt ABI",
        Some(ApplicationErrorKind::PermissionDenied) => "Permission denied",
        Some(ApplicationErrorKind::Conflict) => "Plugin already installed",
        Some(ApplicationErrorKind::Backend) => "Plugin storage failure",
        _ => "Unable to continue",
    }
}
