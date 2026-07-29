use super::OrganizePanelAction;
use crate::shared::theme::AppTheme;
use arclain_theme::spacing;
use eframe::egui;

/// `metadata_title` is the title this session's plugin metadata
/// reported, if any -- `None` renders the no-metadata layout.
pub fn render_header(
    ui: &mut egui::Ui,
    archive_name: &str,
    metadata_title: Option<&str>,
    can_apply: bool,
    theme: &AppTheme,
) -> Option<OrganizePanelAction> {
    let mut action = None;
    egui::Frame::NONE
        .fill(ui.style().visuals.extreme_bg_color)
        .inner_margin(spacing::CARD)
        .corner_radius(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(egui_phosphor::regular::FOLDER_NOTCH_OPEN)
                        .size(28.0)
                        .color(theme.colors.info),
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    arclain_widgets::Text::new("Organize Archive")
                        .size(18.0)
                        .strong()
                        .show(ui);
                    arclain_widgets::Text::new(archive_name)
                        .size(12.0)
                        .muted()
                        .show(ui);
                });

                // Metadata badge - smaller with explicit label
                if let Some(title) = metadata_title {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply button (enabled/disabled based on metadata)
                        let apply_btn = egui::Button::new(
                            egui::RichText::new(format!(
                                "{}  Apply",
                                egui_phosphor::regular::CHECK
                            ))
                            .strong()
                            .size(12.0),
                        )
                        .fill(if can_apply {
                            theme.colors.success
                        } else {
                            theme.colors.outline
                        });

                        if ui.add_enabled(can_apply, apply_btn).clicked() {
                            action = Some(OrganizePanelAction::Apply);
                        }

                        ui.add_space(8.0);

                        egui::Frame::NONE
                            .fill(theme.colors.success.linear_multiply(0.2))
                            .inner_margin(egui::Margin::symmetric(6, 3))
                            .corner_radius(3.0)
                            .show(ui, |ui| {
                                // Use right-to-left layout to ensure tight packing (no stretching)
                                // Add items in REVERSE order: Text then Icon -> appears as [Icon] [Text]
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;

                                        // Text (appears on right)
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Fetched: {}",
                                                super::OrganizePanel::truncate_path(title, 30)
                                            ))
                                            .color(theme.colors.success)
                                            .size(10.0),
                                        );

                                        // Icon (appears on left of text)
                                        ui.label(
                                            egui::RichText::new(
                                                egui_phosphor::regular::CHECK_CIRCLE,
                                            )
                                            .color(theme.colors.success)
                                            .size(12.0),
                                        );
                                    },
                                );
                            });
                    });
                } else {
                    // No metadata - show Apply button (possibly disabled)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let apply_btn = egui::Button::new(
                            egui::RichText::new(format!(
                                "{}  Apply",
                                egui_phosphor::regular::CHECK
                            ))
                            .strong()
                            .size(12.0),
                        )
                        .fill(if can_apply {
                            theme.colors.success
                        } else {
                            theme.colors.outline
                        });

                        if ui.add_enabled(can_apply, apply_btn).clicked() {
                            action = Some(OrganizePanelAction::Apply);
                        }
                    });
                }
            });
        });
    action
}
