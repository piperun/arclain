use super::OrganizePanelAction;
use arclain_core::features::organization::session::OrganizationSession;
use eframe::egui;

pub fn render_header(
    ui: &mut egui::Ui,
    session: &OrganizationSession,
    can_apply: bool,
) -> Option<OrganizePanelAction> {
    let mut action = None;
    egui::Frame::NONE
        .fill(ui.style().visuals.extreme_bg_color)
        .inner_margin(12.0)
        .corner_radius(8.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(egui_phosphor::regular::FOLDER_NOTCH_OPEN)
                        .size(28.0)
                        .color(egui::Color32::from_rgb(99, 179, 237)),
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    arclain_widgets::Text::new("Organize Archive")
                        .size(18.0)
                        .strong()
                        .show(ui);
                    arclain_widgets::Text::new(&session.archive_name)
                        .size(12.0)
                        .muted()
                        .show(ui);
                });

                // Metadata badge - smaller with explicit label
                if let Some(meta) = &session.metadata {
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
                            egui::Color32::from_rgb(34, 139, 34)
                        } else {
                            egui::Color32::from_rgb(60, 60, 60)
                        });

                        if ui.add_enabled(can_apply, apply_btn).clicked() {
                            action = Some(OrganizePanelAction::Apply);
                        }

                        ui.add_space(8.0);

                        // Metadata badge
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(35, 65, 45))
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
                                                super::OrganizePanel::truncate_path(
                                                    &meta.title,
                                                    30
                                                )
                                            ))
                                            .color(egui::Color32::from_rgb(120, 200, 150))
                                            .size(10.0),
                                        );

                                        // Icon (appears on left of text)
                                        ui.label(
                                            egui::RichText::new(
                                                egui_phosphor::regular::CHECK_CIRCLE,
                                            )
                                            .color(egui::Color32::from_rgb(120, 200, 150))
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
                            egui::Color32::from_rgb(34, 139, 34)
                        } else {
                            egui::Color32::from_rgb(60, 60, 60)
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
