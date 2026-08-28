use super::OrganizePanel;
use crate::shared::components::preview_tree::{self, PreviewFilter};
use crate::shared::theme::AppTheme;
use arclain_app::organization::PlannedOutputDto;
use arclain_widgets::ThemedDropdown;
use eframe::egui::{self, RichText};
use egui_extras::{Size, StripBuilder};

/// Vertical space each preview pane reserves for its own chrome before
/// sizing its tree: the frame's 8pt inner margin at top and bottom, the
/// title line, and the separator beneath it.
const PANE_CHROME_HEIGHT: f32 = 40.0;

/// What to call an output on screen. An empty root folder is a real
/// layout -- a lone output with no wrapper, its content at the top level
/// -- and rendering it as an empty string would read as a bug.
fn display_root(output: &PlannedOutputDto) -> &str {
    if output.root_folder.is_empty() {
        "(no wrapper folder)"
    } else {
        &output.root_folder
    }
}

/// One planned output: its resolved folder name, what lands in it, and
/// why it looks the way it does.
///
/// The reasoning is the point rather than decoration. A preview that
/// says what will happen without saying why leaves the user unable to
/// tell a good inference from a bad one -- which folder was taken as the
/// payload and on what evidence is exactly the judgement worth checking.
fn render_output(ui: &mut egui::Ui, theme: &AppTheme, index: usize, output: &PlannedOutputDto) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(egui_phosphor::regular::FOLDER_OPEN).color(theme.colors.warning));
        ui.add(
            egui::Label::new(
                RichText::new(display_root(output))
                    .monospace()
                    .color(theme.colors.info),
            )
            .truncate(),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(RichText::new(format!("{} Copy", egui_phosphor::regular::COPY)).size(11.0))
                .on_hover_text("Copy folder name to clipboard")
                .clicked()
            {
                ui.ctx().copy_text(output.root_folder.clone());
            }
        });
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "{} moved, {} generated, {} fetched",
                output.moves.len(),
                output.generated_files.len(),
                output.downloads.len()
            ))
            .size(11.0)
            .weak(),
        );
        if output.root_folder_template != output.root_folder {
            ui.label(
                RichText::new(format!("from {}", output.root_folder_template))
                    .monospace()
                    .size(11.0)
                    .weak(),
            );
        }
    });

    if !output.reasoning.is_empty() {
        // Salted by position, so two outputs of one plan do not share a
        // collapsing state and toggle together.
        egui::CollapsingHeader::new(RichText::new("Why").size(11.0))
            .id_salt(("organize_output_reasoning", index))
            .default_open(false)
            .show(ui, |ui| {
                for line in &output.reasoning {
                    ui.label(RichText::new(line).size(11.0).weak());
                }
            });
    }
}

impl OrganizePanel {
    pub(super) fn render_preview_tab(&mut self, ui: &mut egui::Ui, theme: &AppTheme) {
        // Destructured rather than cloned: the plan is read here while
        // `ui_state` is written, and the two are disjoint fields. A
        // clone would copy every planned move, download and resolved
        // variable once per rendered frame.
        let Self {
            archive_name,
            metadata,
            preview,
            ui_state,
            ..
        } = self;
        if let Some(plan) = preview.as_ref() {
            egui::Frame::NONE
                .fill(theme.colors.surface_variant)
                .inner_margin(10.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(egui_phosphor::regular::FOLDER)
                                .color(theme.colors.warning),
                        );
                        ui.label(
                            RichText::new(match plan.outputs.len() {
                                1 => "Output:".to_string(),
                                count => format!("{count} outputs:"),
                            })
                            .strong(),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Export Tree Button
                            if ui
                                .button(RichText::new(format!(
                                    "{} Export Tree",
                                    egui_phosphor::regular::EXPORT
                                )))
                                .clicked()
                            {
                                ui_state.export_dialog.open();
                            }
                        });
                    });

                    // Every output, never just the first: an archive is
                    // not one folder, and a panel that silently showed
                    // one of three would describe a run the user is not
                    // about to get.
                    if plan.outputs.is_empty() {
                        ui.label(
                            RichText::new("this rule produces no output folder for this archive")
                                .color(theme.colors.warning),
                        );
                    }
                    for (index, output) in plan.outputs.iter().enumerate() {
                        render_output(ui, theme, index, output);
                    }

                    // A folder the plan passed over is not an error, but
                    // it is the only place the user learns it happened.
                    for skipped in &plan.skipped_outputs {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(egui_phosphor::regular::WARNING)
                                    .color(theme.colors.warning),
                            );
                            ui.label(
                                RichText::new(if skipped.root.is_empty() {
                                    "skipped".to_string()
                                } else {
                                    format!("skipped {}", skipped.root)
                                })
                                .monospace()
                                .size(12.0)
                                .color(theme.colors.warning),
                            );
                            ui.label(RichText::new(&skipped.reason).size(12.0).weak());
                        });
                    }
                });

            ui.add_space(4.0);

            // Variables moved to separate tab

            // STATS BAR with INTEGRITY VERIFICATION
            let report = &plan.integrity;

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

                // Discrepancy warning.
                //
                // `file_discrepancy` is carried through from the
                // application verbatim, including the fact that the
                // computation behind it can never produce a positive
                // value -- so the "filtered out" branch below is
                // currently unreachable and a shortfall renders as a
                // green "N added". Both are pinned by the pending fix to
                // that computation: correcting the sign here alone would
                // fork the bug rather than close it.
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
                            theme.colors.warning
                        } else {
                            theme.colors.success
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
                        .color(theme.colors.warning),
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
                            .color(theme.colors.success),
                    )
                    .on_hover_text(format!(
                        "Source content verified (invariant under move)\nOriginal Hash: {:016x}\nResult Hash:   {:016x}",
                        report.original_hash, report.result_hash
                    ));
                } else {
                    ui.label(
                        RichText::new(format!("{} Mismatch", egui_phosphor::regular::X_CIRCLE))
                            .size(11.0)
                            .color(theme.colors.error),
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
                        super::integrity::export_issues_report(
                            report,
                            &ui_state.original_tree,
                            &ui_state.organized_tree,
                            metadata.as_ref(),
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
                            ui_state.preview_filter == filter,
                            RichText::new(label).size(11.0),
                        )
                        .clicked()
                    {
                        ui_state.preview_filter = filter;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ThemedDropdown::new(
                        "depth_limit",
                        match ui_state.depth_limit {
                            None => "Depth: All".to_string(),
                            Some(0) => "Depth: Root".to_string(),
                            Some(n) => format!("Depth: {}", n),
                        },
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut ui_state.depth_limit, None, "All");
                        ui.selectable_value(&mut ui_state.depth_limit, Some(0), "Root Only");
                        ui.selectable_value(&mut ui_state.depth_limit, Some(1), "1 Level");
                        ui.selectable_value(&mut ui_state.depth_limit, Some(2), "2 Levels");
                        ui.selectable_value(&mut ui_state.depth_limit, Some(3), "3 Levels");
                    });
                });
            });

            ui.separator();

            // DUAL PANE TREE VIEW
            let available = ui.available_size();
            // What each pane gives up to its own chrome: the frame's 8pt
            // margin top and bottom, the title line, and the separator
            // under it. `set_height` sets a maximum as well as a minimum,
            // so a pane that did not reserve this would size itself to
            // the whole cell and then overflow it by its own margins.
            //
            // Clamped because a window can be shorter than the chrome:
            // egui takes a negative height as a bug and, in a build with
            // debug assertions, panics rather than clipping.
            let pane_height = (available.y - PANE_CHROME_HEIGHT).max(0.0);

            StripBuilder::new(ui)
                .size(Size::remainder().at_least(100.0)) // Left Pane
                .size(Size::exact(30.0)) // Arrow
                .size(Size::remainder().at_least(100.0)) // Right Pane
                .horizontal(|mut strip| {
                    // LEFT PANE: Original structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(theme.colors.surface_variant)
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(pane_height);

                                    let original_title = format!("Original: {}", archive_name);
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
                                                &mut ui_state.original_tree_state,
                                                &ui_state.original_tree,
                                                ui_state.preview_filter,
                                                ui_state.depth_limit,
                                            );
                                        });
                                });
                            });
                    });

                    // ARROW
                    strip.cell(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space((available.y / 2.0 - 20.0).max(0.0));
                            ui.label(
                                RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                    .size(20.0)
                                    .color(theme.colors.success),
                            );
                        });
                    });

                    // RIGHT PANE: Organized structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(theme.colors.surface_variant)
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(pane_height);

                                    let organized_title = match plan.outputs.as_slice() {
                                        [] => "Modified: nothing".to_string(),
                                        [only] => format!("Modified: {}", display_root(only)),
                                        outputs => {
                                            format!("Modified: {} folders", outputs.len())
                                        }
                                    };
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
                                                &mut ui_state.organized_tree_state,
                                                &ui_state.organized_tree,
                                                ui_state.preview_filter,
                                                ui_state.depth_limit,
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
                        .color(theme.colors.warning),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("No preview available").size(14.0).weak());
            });
        }
    }
}
