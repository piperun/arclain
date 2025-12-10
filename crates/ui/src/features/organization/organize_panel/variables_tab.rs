use super::OrganizePanel;
use eframe::egui::{self, RichText};

impl OrganizePanel {
    pub(super) fn render_variables_tab(&self, ui: &mut egui::Ui) {
        if let Some(plan) = &self.preview_plan {
            ui.vertical(|ui| {
                ui.add_space(8.0);

                // Pattern Header
                egui::Frame::NONE
                    .fill(ui.style().visuals.faint_bg_color)
                    .inner_margin(12.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Pattern:").strong().size(12.0));
                            ui.label(
                                RichText::new(&plan.root_folder_template)
                                    .monospace()
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(250, 204, 21)),
                            );
                        });

                        ui.add_space(8.0);

                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Result:").strong().size(12.0));
                            ui.label(
                                RichText::new(&plan.root_folder)
                                    .monospace()
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(134, 239, 172)),
                            );
                        });
                    });

                ui.add_space(16.0);
                ui.label(RichText::new("Resolved Variables").strong().size(14.0));
                ui.add_space(8.0);

                // Variables Table
                egui::Frame::NONE
                    .fill(ui.style().visuals.extreme_bg_color)
                    .inner_margin(8.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("variables_scroll")
                            .show(ui, |ui| {
                                egui::Grid::new("variables_tab_grid")
                                    .num_columns(2)
                                    .spacing([20.0, 12.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        // Sort keys for consistent display
                                        let mut keys: Vec<_> =
                                            plan.resolved_variables.keys().collect();
                                        keys.sort();

                                        for key in keys {
                                            if let Some(value) = plan.resolved_variables.get(key) {
                                                ui.label(
                                                    RichText::new(format!("${}", key))
                                                        .monospace()
                                                        .size(12.0)
                                                        .strong()
                                                        .color(
                                                            ui.style()
                                                                .visuals
                                                                .text_color()
                                                                .gamma_multiply(0.8),
                                                        ),
                                                );

                                                // Wrap long values
                                                ui.label(
                                                    RichText::new(value)
                                                        .monospace()
                                                        .size(12.0)
                                                        .color(egui::Color32::from_rgb(
                                                            147, 197, 253,
                                                        )),
                                                );
                                                ui.end_row();
                                            }
                                        }
                                    });
                            });
                    });
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No plan loaded").weak());
            });
        }
    }
}
