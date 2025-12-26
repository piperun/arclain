use super::OrganizePanel;
use crate::shared::components::preview_tree::{self, PreviewFilter};
use eframe::egui::{self, RichText};
use egui_extras::{Size, StripBuilder};

impl OrganizePanel {
    pub(super) fn render_preview_tab(&mut self, ui: &mut egui::Ui) {
        if let Some(plan) = &self.session.preview_plan.clone() {
            // HEADER: Output folder with copy button
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(35, 45, 55))
                .inner_margin(10.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(egui_phosphor::regular::FOLDER)
                                .color(egui::Color32::from_rgb(250, 204, 21)),
                        );
                        ui.label(RichText::new("Output:").strong());
                        ui.label(
                            RichText::new(&plan.root_folder)
                                .monospace()
                                .color(egui::Color32::from_rgb(147, 197, 253)),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Copy button
                            if ui
                                .button(RichText::new(format!(
                                    "{} Copy",
                                    egui_phosphor::regular::COPY
                                )))
                                .on_hover_text("Copy folder name to clipboard")
                                .clicked()
                            {
                                ui.ctx().copy_text(plan.root_folder.clone());
                            }

                            ui.add_space(8.0);

                            // Export Tree Button
                            if ui
                                .button(RichText::new(format!(
                                    "{} Export Tree",
                                    egui_phosphor::regular::EXPORT
                                )))
                                .clicked()
                            {
                                self.ui_state.export_dialog.open();
                            }
                        });
                    });
                });

            ui.add_space(4.0);

            // Variables moved to separate tab

            // STATS BAR with INTEGRITY VERIFICATION
            let report = self.session.calculate_report();

            ui.horizontal(|ui| {
                // Original stats
                ui.label(
                    RichText::new(format!(
                        "{} Original: {} files, {} folders",
                        egui_phosphor::regular::ARCHIVE,
                        report.original_files,
                        report.original_folders
                    ))
                    .size(11.0)
                    .weak(),
                );

                ui.separator();

                // Modified stats
                ui.label(
                    RichText::new(format!(
                        "{} Modified: {} files ({} moved + {} gen + {} dl)",
                        egui_phosphor::regular::FOLDER_NOTCH_OPEN,
                        report.expected_modified_files,
                        report.moved_files,
                        report.generated_files,
                        report.planned_screenshots
                    ))
                    .size(11.0)
                    .weak(),
                );

                // Discrepancy warning
                if report.file_discrepancy != 0 {
                    ui.separator();
                    let discrepancy_text = if report.file_discrepancy > 0 {
                        format!(
                            "{} {} filtered out",
                            egui_phosphor::regular::WARNING,
                            report.file_discrepancy
                        )
                    } else {
                        format!(
                            "{} {} added",
                            egui_phosphor::regular::PLUS,
                            -report.file_discrepancy
                        )
                    };
                    ui.label(RichText::new(discrepancy_text).size(11.0).color(
                        if report.file_discrepancy > 0 {
                            egui::Color32::from_rgb(251, 191, 36) // Warning yellow
                        } else {
                            egui::Color32::from_rgb(74, 222, 128) // Success green
                        },
                    ));
                }

                // Screenshot warning
                if report.expected_screenshots != report.planned_screenshots {
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} Screenshots: {}/{} planned",
                            egui_phosphor::regular::IMAGE,
                            report.planned_screenshots,
                            report.expected_screenshots
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                    )
                    .on_hover_text("Some screenshots may not be available or failed to load");
                }

                // Fingerprint match indicator
                ui.separator();
                // Content Hash / Coverage match indicator
                ui.separator();
                if report.content_match {
                    ui.label(
                        RichText::new(format!("{} Verified", egui_phosphor::regular::CHECK_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(74, 222, 128)), // Green
                    )
                    .on_hover_text(format!(
                        "Source content verified (invariant under move)\nOriginal Hash: {:016x}\nResult Hash:   {:016x}",
                        report.original_hash, report.result_hash
                    ));
                } else {
                    ui.label(
                        RichText::new(format!("{} Mismatch", egui_phosphor::regular::X_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(248, 113, 113)), // Red
                    )
                    .on_hover_text(format!(
                        "Hash mismatch! Files missing or added.\nOriginal Hash: {:016x}\nResult Hash:   {:016x}\nMissing: {} file(s)",
                        report.original_hash, report.result_hash, report.missing_original_files.len()
                    ));
                }
            });

            ui.horizontal(|ui| {
                // Load Screenshots button removed per user request

                // Export Issues button - visible when there are discrepancies
                if report.file_discrepancy > 0
                    || report.expected_screenshots != report.planned_screenshots
                {
                    ui.separator();
                    if ui
                        .button(format!(
                            "{} Export Issues",
                            egui_phosphor::regular::WARNING_CIRCLE
                        ))
                        .on_hover_text(
                            "Export a report of files filtered out and missing screenshots",
                        )
                        .clicked()
                    {
                        Self::export_issues_report(
                            &report,
                            &self.ui_state.original_tree,
                            &self.ui_state.organized_tree,
                            &self.session.metadata,
                        );
                    }
                }
            });

            ui.add_space(4.0);

            // FILTER TABS & DEPTH LIMIT
            ui.horizontal(|ui| {
                let filters = [
                    (PreviewFilter::All, "All"),
                    (PreviewFilter::FoldersOnly, "📁 Folders"),
                    (PreviewFilter::FilesOnly, "📄 Files"),
                    (PreviewFilter::GeneratedOnly, "✨ Generated"),
                ];
                for (filter, label) in filters {
                    if ui
                        .selectable_label(
                            self.ui_state.preview_filter == filter,
                            RichText::new(label).size(11.0),
                        )
                        .clicked()
                    {
                        self.ui_state.preview_filter = filter;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("depth_limit")
                        .selected_text(match self.ui_state.depth_limit {
                            None => "Depth: All".to_string(),
                            Some(0) => "Depth: Root".to_string(),
                            Some(n) => format!("Depth: {}", n),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.ui_state.depth_limit, None, "All");
                            ui.selectable_value(
                                &mut self.ui_state.depth_limit,
                                Some(0),
                                "Root Only",
                            );
                            ui.selectable_value(&mut self.ui_state.depth_limit, Some(1), "1 Level");
                            ui.selectable_value(
                                &mut self.ui_state.depth_limit,
                                Some(2),
                                "2 Levels",
                            );
                            ui.selectable_value(
                                &mut self.ui_state.depth_limit,
                                Some(3),
                                "3 Levels",
                            );
                        });
                });
            });

            ui.separator();

            // DUAL PANE TREE VIEW
            let available = ui.available_size();

            StripBuilder::new(ui)
                .size(Size::remainder().at_least(100.0)) // Left Pane
                .size(Size::exact(30.0)) // Arrow
                .size(Size::remainder().at_least(100.0)) // Right Pane
                .horizontal(|mut strip| {
                    // LEFT PANE: Original structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(30, 30, 35))
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(available.y - 40.0);

                                    let original_title =
                                        format!("Original: {}", self.session.archive_name);
                                    ui.add(
                                        egui::Label::new(
                                            arclain_widgets::Text::new(&original_title)
                                                .strong()
                                                .size(12.0)
                                                .to_rich_text(ui),
                                        )
                                        .truncate(),
                                    );
                                    ui.separator();

                                    egui::ScrollArea::both()
                                        .id_salt("original_tree")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            preview_tree::render_tree(
                                                ui,
                                                &mut self.ui_state.original_tree_state,
                                                &self.ui_state.original_tree,
                                                self.ui_state.preview_filter,
                                                self.ui_state.depth_limit,
                                            );
                                        });
                                });
                            });
                    });

                    // ARROW
                    strip.cell(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(available.y / 2.0 - 20.0);
                            ui.label(
                                RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                    .size(20.0)
                                    .color(egui::Color32::from_rgb(74, 222, 128)),
                            );
                        });
                    });

                    // RIGHT PANE: Organized structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(30, 35, 35))
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(available.y - 40.0);

                                    let organized_title = format!("Modified: {}", plan.root_folder);
                                    ui.add(
                                        egui::Label::new(
                                            arclain_widgets::Text::new(&organized_title)
                                                .strong()
                                                .size(12.0)
                                                .to_rich_text(ui),
                                        )
                                        .truncate(),
                                    );
                                    ui.separator();

                                    egui::ScrollArea::both()
                                        .id_salt("organized_tree")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            preview_tree::render_tree(
                                                ui,
                                                &mut self.ui_state.organized_tree_state,
                                                &self.ui_state.organized_tree,
                                                self.ui_state.preview_filter,
                                                self.ui_state.depth_limit,
                                            );
                                        });
                                });
                            });
                    });
                });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new(egui_phosphor::regular::WARNING)
                        .size(40.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("No preview available").size(14.0).weak());
            });
        }
    }
}
