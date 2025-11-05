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
            // Use hover so it doesn't capture drags/wheel unnecessarily
            ui.allocate_rect(screen, egui::Sense::hover());
        });

    // Regex tester modal
    egui::Area::new(egui::Id::new("regex_tester_modal"))
        // Important: draw on the same highest layer as the overlay so that
        // the modal appears above the dim. The overlay is painted first (see
        // above), then this modal is painted second on the same layer, which
        // ensures it is visually on top.
        .order(egui::Order::Foreground)
        .interactable(true)
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
                egui::CornerRadius::same(8),
                egui::Stroke::new(1.0, theme.colors.border_color),
                egui::StrokeKind::Outside,
            );

            // Make modal rect interactive to receive hover/wheel
            let _modal_rect_resp = ui.allocate_rect(rect, egui::Sense::hover());
            // Clip all modal content to the modal rectangle so hover/scroll goes to it
            ui.set_clip_rect(rect);

            let content = rect.shrink2(egui::vec2(20.0, 16.0));
            // Reserve space for a non-scrollable bottom bar
            let bottom_bar_h = 44.0;
            let scroll_rect = egui::Rect::from_min_max(
                content.min,
                egui::pos2(content.max.x, content.max.y - bottom_bar_h - 8.0),
            );
            let bottom_rect = egui::Rect::from_min_max(
                egui::pos2(content.min.x, content.max.y - bottom_bar_h),
                content.max,
            );
            // Ensure the scroll viewport participates in input so wheel events can be captured by children
            let _content_resp = ui.allocate_rect(scroll_rect, egui::Sense::hover());
            // Main scrollable content area
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(scroll_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
 
            // Clip content strictly to the scroll viewport so it cannot draw into the bottom bar
            child.set_clip_rect(scroll_rect);
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
 
                // Calculate available height for scroll area (remaining height in scroll_rect)
                let available_height = scroll_rect.height() - ui.min_rect().height() + scroll_rect.min.y;
 
                egui::ScrollArea::vertical()
                    .max_height(available_height)
                    .auto_shrink([false, false])
                    .id_salt("regex_tester_results_scroll")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                                // Disable text wrapping within this scroll area so long
                                // lines produce horizontal scrolling instead of spilling
                                // out or wrapping unexpectedly.
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
                                                    .family(egui::FontFamily::Monospace)
                                                    .color(if result.matched {
                                                        theme.colors.text_primary
                                                    } else {
                                                        theme.colors.text_secondary
                                                    }),
                                            );
                                        });
                                    }
                                }
// small end padding to avoid tight edge at bottom of the viewport
ui.add_space(4.0);
                            });
            });
 
            // Fixed bottom action bar
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bottom_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            bar.horizontal(|ui| {
                let bar_rect = ui.max_rect();
                // solid background for fixed bar
                ui.painter().rect_filled(bar_rect, 0.0, theme.colors.bg_primary);
                // subtle top separator
                let sep_rect = egui::Rect::from_min_max(
                    egui::pos2(bar_rect.min.x, bar_rect.min.y),
                    egui::pos2(bar_rect.max.x, bar_rect.min.y + 1.0),
                );
                ui.painter()
                    .rect_filled(sep_rect, 0.0, theme.colors.border_color);
 
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Close").strong())
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