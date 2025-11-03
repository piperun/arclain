// Regex tester modal for Password Rules
use crate::features::theme::AppTheme;
use eframe::egui;

use super::state::PasswordRulesDialog;

pub fn render_regex_tester_modal(
    ctx: &egui::Context,
    theme: &AppTheme,
    dialog: &mut PasswordRulesDialog,
) {
    // Dim overlay for the tester modal - capture all input to block background interaction
    // Keep this on Middle so it sits above app content but below the tester modal
    // (which is on Tooltip), ensuring input goes to the modal and not the overlay.
    egui::Area::new(egui::Id::new("regex_tester_overlay"))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(200));
            // Sense all input on the overlay to block interaction with content behind it
            ui.allocate_rect(screen, egui::Sense::click_and_drag());
        });

    // Regex tester modal
    egui::Area::new(egui::Id::new("regex_tester_modal"))
        // Important: draw on the same highest layer as the overlay so that
        // the modal appears above the dim. The overlay is painted first (see
        // above), then this modal is painted second on the same layer, which
        // ensures it is visually on top.
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            let screen = ctx.screen_rect();
            let width = (screen.width() * 0.5).clamp(500.0, 700.0);
            let height = (screen.height() * 0.6).clamp(400.0, 600.0);
            let pos = egui::pos2(
                (screen.width() - width) / 2.0,
                (screen.height() - height) / 2.0,
            );
            let rect = egui::Rect::from_min_size(pos, egui::vec2(width, height));

            ui.painter().rect_filled(rect, 8.0, theme.colors.bg_primary);
            ui.painter().rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(1.0, theme.colors.border_color),
            );

            let content = rect.shrink2(egui::vec2(20.0, 16.0));
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );

            child.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

                // Title
                ui.label(
                    egui::RichText::new("🧪 Regex Pattern Tester")
                        .size(16.0)
                        .strong()
                        .color(theme.colors.text_primary),
                );

                ui.add_space(8.0);

                // Pattern display
                ui.label(
                    egui::RichText::new("Testing pattern:")
                        .size(12.0)
                        .color(theme.colors.text_secondary),
                );
                ui.label(
                    egui::RichText::new(&dialog.regex_test_pattern)
                        .size(13.0)
                        .family(egui::FontFamily::Monospace)
                        .color(theme.colors.text_primary)
                        .background_color(theme.colors.bg_tertiary),
                );

                ui.add_space(8.0);

                // Folder picker
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Test folder:")
                            .size(12.0)
                            .color(theme.colors.text_secondary),
                    );
                    if let Some(folder) = &dialog.regex_test_folder {
                        ui.label(
                            egui::RichText::new(folder.display().to_string())
                                .size(11.0)
                                .color(theme.colors.text_secondary),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("No folder selected")
                                .size(11.0)
                                .italics()
                                .color(theme.colors.text_secondary),
                        );
                    }
                });

                ui.horizontal(|ui| {
                    if ui.button("📁 Pick Folder").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            dialog.regex_test_folder = Some(folder.clone());

                            // Test the regex against all files in the folder
                            if let Ok(entries) = std::fs::read_dir(&folder) {
                                dialog.regex_test_results.clear();

                                // Convert glob pattern to regex
                                let pattern_str = dialog
                                    .regex_test_pattern
                                    .replace(".", "\\.")
                                    .replace("*", ".*")
                                    .replace("?", ".");

                                if let Ok(re) = regex::Regex::new(&pattern_str) {
                                    for entry in entries.flatten() {
                                        if let Ok(file_type) = entry.file_type() {
                                            if file_type.is_file() {
                                                let file_name = entry.file_name();
                                                let file_name_str =
                                                    file_name.to_string_lossy().to_string();
                                                let matched = re.is_match(&file_name_str);

                                                dialog.regex_test_results.push(
                                                    super::types::RegexTestResult {
                                                        file_path: file_name_str,
                                                        matched,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if ui.button("🔄 Refresh").clicked() {
                        if let Some(folder) = &dialog.regex_test_folder {
                            if let Ok(entries) = std::fs::read_dir(folder) {
                                dialog.regex_test_results.clear();

                                let pattern_str = dialog
                                    .regex_test_pattern
                                    .replace(".", "\\.")
                                    .replace("*", ".*")
                                    .replace("?", ".");

                                if let Ok(re) = regex::Regex::new(&pattern_str) {
                                    for entry in entries.flatten() {
                                        if let Ok(file_type) = entry.file_type() {
                                            if file_type.is_file() {
                                                let file_name = entry.file_name();
                                                let file_name_str =
                                                    file_name.to_string_lossy().to_string();
                                                let matched = re.is_match(&file_name_str);

                                                dialog.regex_test_results.push(
                                                    super::types::RegexTestResult {
                                                        file_path: file_name_str,
                                                        matched,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                });

                ui.add_space(8.0);

                // Results
                ui.label(
                    egui::RichText::new(format!(
                        "Results ({} files tested):",
                        dialog.regex_test_results.len()
                    ))
                    .size(12.0)
                    .color(theme.colors.text_secondary),
                );

                egui::ScrollArea::both()
                    .max_height(content.height() - 200.0)
                    .show(ui, |ui| {
                        // Disable text wrapping within this scroll area so long
                        // lines produce horizontal scrolling instead of spilling
                        // out or wrapping unexpectedly.
                        // Use wrap_mode to disable wrapping for labels in this area
                        ui.style_mut().wrap_mode =
                            Some(eframe::egui::TextWrapMode::Extend);
                        if dialog.regex_test_results.is_empty() {
                            ui.label(
                                egui::RichText::new(
                                    "Pick a folder to test the pattern",
                                )
                                .size(12.0)
                                .italics()
                                .color(theme.colors.text_secondary),
                            );
                        } else {
                            for result in &dialog.regex_test_results {
                                ui.horizontal(|ui| {
                                    let (icon, color) = if result.matched {
                                        ("✓", egui::Color32::from_rgb(76, 175, 80))
                                    } else {
                                        ("✗", egui::Color32::from_rgb(244, 67, 54))
                                    };

                                    ui.label(
                                        egui::RichText::new(icon)
                                            .size(12.0)
                                            .color(color),
                                    );
                                    ui.label(
                                        egui::RichText::new(&result.file_path)
                                            .size(11.0)
                                            .family(
                                                egui::FontFamily::Monospace,
                                            )
                                            .color(if result.matched {
                                                theme.colors.text_primary
                                            } else {
                                                theme.colors.text_secondary
                                            }),
                                    );
                                });
                            }
                        }
                    });

                ui.add_space(10.0);

                // Close button
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Close").strong(),
                                )
                                .min_size(egui::vec2(100.0, 32.0)),
                            )
                            .clicked()
                        {
                            dialog.show_regex_tester = false;
                            dialog.regex_test_results.clear();
                        }
                    },
                );
            });
        });
}