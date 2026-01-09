//! Breadcrumb navigation for file list

use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render breadcrumb navigation showing current path
pub fn render_breadcrumb(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    current_path: &str,
    archive_name: &str,
) -> Option<String> {
    let mut navigate_to: Option<String> = None;
    let available_width = ui.available_width();
    let default_font = egui::FontId::proportional(14.0);

    // Estimate width of root button
    let root_galley = ui.painter().layout_no_wrap(
        archive_name.to_string(),
        default_font.clone(),
        theme.colors.on_surface,
    );
    let root_width = root_galley.rect.width() + 16.0; // padding

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);

        // Root archive button (clickable)
        let root_response = ui.add(
            egui::Label::new(
                egui::RichText::new(archive_name)
                    .size(14.0)
                    .color(theme.colors.on_surface),
            )
            .selectable(false)
            .sense(egui::Sense::click()),
        );

        if root_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let rect = root_response.rect;
            ui.painter().line_segment(
                [
                    egui::pos2(rect.min.x, rect.max.y),
                    egui::pos2(rect.max.x, rect.max.y),
                ],
                egui::Stroke::new(1.0, theme.colors.on_surface),
            );
        }

        if root_response.clicked() {
            navigate_to = Some(String::new());
        }

        if !current_path.is_empty() {
            // Separator after root
            ui.label(
                egui::RichText::new("/")
                    .size(14.0)
                    .color(theme.colors.on_surface_variant),
            );

            let segments: Vec<&str> = current_path.split('/').collect();
            let separator_width = 12.0; // approx width of " / "

            // Calculate widths of all segments
            let segment_widths: Vec<f32> = segments
                .iter()
                .map(|s| {
                    ui.painter()
                        .layout_no_wrap(
                            s.to_string(),
                            default_font.clone(),
                            theme.colors.on_surface,
                        )
                        .rect
                        .width()
                })
                .collect();

            let total_segments_width: f32 = segment_widths.iter().sum::<f32>()
                + (segment_widths.len() as f32 * separator_width);

            // Determine if we need to compact
            let path_available_width = available_width - root_width;

            if total_segments_width <= path_available_width || segments.len() <= 2 {
                // Render all segments normally
                for (idx, segment) in segments.iter().enumerate() {
                    if idx > 0 {
                        ui.label(
                            egui::RichText::new("/")
                                .size(14.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    }
                    render_breadcrumb_segment(
                        ui,
                        theme,
                        segment,
                        idx == segments.len() - 1,
                        idx,
                        &segments,
                        &mut navigate_to,
                    );
                }
            } else {
                // COMPACT MODE: Show ... / grandparent / parent / current
                let mut kept_indices = std::collections::VecDeque::new();
                let mut used_width = 0.0;
                let ellipsis_width = 24.0;

                // Always show last one
                if let Some(last_idx) = segments.len().checked_sub(1) {
                    kept_indices.push_front(last_idx);
                    used_width += segment_widths[last_idx];
                }

                // Try adding more from the end backwards
                for idx in (0..segments.len() - 1).rev() {
                    let w = segment_widths[idx] + separator_width;
                    if used_width + w + ellipsis_width + separator_width < path_available_width {
                        kept_indices.push_front(idx);
                        used_width += w;
                    } else {
                        break;
                    }
                }

                // Check if we need ellipsis
                let first_visible = kept_indices.front().copied().unwrap_or(0);
                let show_ellipsis = first_visible > 0;

                if show_ellipsis {
                    let hidden_segments: Vec<(usize, &str)> = segments[0..first_visible]
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (i, *s))
                        .collect();

                    let ellipsis_popup_id = ui.make_persistent_id("breadcrumb_ellipsis_popup");
                    let ellipsis_response = ui.add(
                        egui::Label::new(
                            egui::RichText::new("...")
                                .size(14.0)
                                .color(theme.colors.on_surface_variant),
                        )
                        .selectable(false)
                        .sense(egui::Sense::click()),
                    );

                    if ellipsis_response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    if ellipsis_response.clicked() {
                        egui::Popup::toggle_id(ui.ctx(), ellipsis_popup_id);
                    }

                    #[allow(deprecated)]
                    egui::popup_below_widget(
                        ui,
                        ellipsis_popup_id,
                        &ellipsis_response,
                        egui::PopupCloseBehavior::CloseOnClickOutside,
                        |ui| {
                            ui.set_min_width(150.0);
                            for (idx, segment) in hidden_segments.iter() {
                                let full_path = segments[0..=*idx].join("/");
                                if ui.button(*segment).clicked() {
                                    navigate_to = Some(full_path);
                                }
                            }
                        },
                    );

                    ui.label(
                        egui::RichText::new("/")
                            .size(14.0)
                            .color(theme.colors.on_surface_variant),
                    );
                }

                for (i, &idx) in kept_indices.iter().enumerate() {
                    if i > 0 {
                        ui.label(
                            egui::RichText::new("/")
                                .size(14.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    }
                    render_breadcrumb_segment(
                        ui,
                        theme,
                        segments[idx],
                        idx == segments.len() - 1,
                        idx,
                        &segments,
                        &mut navigate_to,
                    );
                }
            }
        }
    });

    navigate_to
}

fn render_breadcrumb_segment(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    segment: &str,
    is_last: bool,
    idx: usize,
    all_segments: &[&str],
    navigate_to: &mut Option<String>,
) {
    let text_color = if is_last {
        theme.colors.on_surface
    } else {
        theme.colors.on_surface_variant
    };

    let response = ui.add(
        egui::Label::new(egui::RichText::new(segment).size(14.0).color(text_color))
            .selectable(false)
            .sense(egui::Sense::click()),
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let rect = response.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.min.x, rect.max.y),
                egui::pos2(rect.max.x, rect.max.y),
            ],
            egui::Stroke::new(1.0, text_color),
        );
    }

    if response.clicked() {
        let target = all_segments[..=idx].join("/");
        *navigate_to = Some(target);
    }
}
