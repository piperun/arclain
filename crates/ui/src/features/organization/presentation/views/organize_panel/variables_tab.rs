use super::OrganizePanel;
use crate::shared::theme::AppTheme;
use arclain_theme::spacing;
use eframe::egui::{self, RichText};

impl OrganizePanel {
    pub(super) fn render_variables_tab(&self, ui: &mut egui::Ui, theme: &AppTheme) {
        if let Some(plan) = self.preview() {
            ui.vertical(|ui| {
                ui.add_space(8.0);

                // Variables are resolved per output: two mods of one
                // pack read two different `modinfo.ini` files and so
                // resolve two different `$mod_name`s. One merged table
                // would have to pick a winner, and would then be lying
                // about the other output.
                egui::ScrollArea::vertical()
                    .id_salt("variables_scroll")
                    .show(ui, |ui| {
                        for (index, output) in plan.outputs.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(16.0);
                            }

                            // Pattern Header
                            egui::Frame::NONE
                                .fill(ui.style().visuals.faint_bg_color)
                                .inner_margin(spacing::CARD)
                                .corner_radius(6.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Pattern:").strong().size(12.0));
                                        ui.label(
                                            RichText::new(&output.root_folder_template)
                                                .monospace()
                                                .size(12.0)
                                                .color(theme.colors.warning),
                                        );
                                    });

                                    ui.add_space(8.0);

                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("Result:").strong().size(12.0));
                                        ui.label(
                                            RichText::new(if output.root_folder.is_empty() {
                                                "(no wrapper folder)"
                                            } else {
                                                &output.root_folder
                                            })
                                            .monospace()
                                            .size(12.0)
                                            .color(theme.colors.success),
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
                                    egui::Grid::new(("variables_tab_grid", index))
                                        .num_columns(2)
                                        .spacing([20.0, 12.0])
                                        .striped(true)
                                        .show(ui, |ui| {
                                            // Already sorted by name
                                            // where the plan is computed,
                                            // so two previews of one plan
                                            // render identically.
                                            for variable in &output.resolved_variables {
                                                ui.label(
                                                    RichText::new(format!("${}", variable.name))
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
                                                    RichText::new(&variable.value)
                                                        .monospace()
                                                        .size(12.0)
                                                        .color(theme.colors.info),
                                                );
                                                ui.end_row();
                                            }
                                        });
                                });
                        }
                    });
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No plan loaded").weak());
            });
        }
    }
}
