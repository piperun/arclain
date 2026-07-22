//! A segmented control widget for selecting between two options.
//! Similar to iOS UISegmentedControl - shows two text labels side by side.

use arclain_theme::ThemeColors;
use egui::{Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget};

/// A segmented control for selecting between two text options.
pub struct SegmentedControl<'a> {
    selected: &'a mut bool,
    label_true: String,
    label_false: String,
    width: f32,
    height: f32,
    theme_colors: Option<&'a ThemeColors>,
    debug_lines: bool,
}

impl<'a> SegmentedControl<'a> {
    /// Create a new segmented control.
    /// `selected` is true when the first option is selected, false for the second.
    pub fn new(
        selected: &'a mut bool,
        label_true: impl Into<String>,
        label_false: impl Into<String>,
    ) -> Self {
        Self {
            selected,
            label_true: label_true.into(),
            label_false: label_false.into(),
            width: 120.0,
            height: 28.0,
            theme_colors: None,
            debug_lines: false,
        }
    }

    /// Set custom size.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Apply theme colors.
    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
        self
    }

    /// Force the debug overlay on. ORs with `EGUI_UI_DEBUG_GUIDELINES`.
    /// Stripped in release builds.
    pub fn debug_lines(mut self, on: bool) -> Self {
        self.debug_lines = on;
        self
    }
}

impl<'a> Widget for SegmentedControl<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(self.width, self.height), Sense::click());

        // Determine which half was clicked
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let mid_x = rect.center().x;
                let clicked_left = pos.x < mid_x;
                if clicked_left != *self.selected {
                    *self.selected = clicked_left;
                    response.mark_changed();
                }
            }
        }

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().visuals.clone();
            let radius: f32 = 4.0;
            let corner_radius = CornerRadius::same(radius.round() as u8);

            // Colors
            let bg_color = self
                .theme_colors
                .map(|c| c.surface_variant)
                .unwrap_or(visuals.widgets.inactive.bg_fill);
            let selected_bg = self
                .theme_colors
                .map(|c| c.primary)
                .unwrap_or(visuals.selection.bg_fill);
            let text_color = self
                .theme_colors
                .map(|c| c.on_surface)
                .unwrap_or(visuals.text_color());
            let selected_text_color = self
                .theme_colors
                .map(|c| c.on_primary)
                .unwrap_or(Color32::WHITE);

            // Draw background
            let stroke = Stroke::new(1.0_f32, bg_color.linear_multiply(0.8));
            ui.painter()
                .rect(rect, corner_radius, bg_color, stroke, StrokeKind::Middle);

            // Left half
            let left_rect =
                egui::Rect::from_min_size(rect.min, Vec2::new(self.width / 2.0, self.height));
            // Right half
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + self.width / 2.0, rect.min.y),
                Vec2::new(self.width / 2.0, self.height),
            );

            // Draw selected background
            let (sel_rect, sel_text, sel_text_color, unsel_text, unsel_text_color) =
                if *self.selected {
                    (
                        left_rect,
                        &self.label_true,
                        selected_text_color,
                        &self.label_false,
                        text_color,
                    )
                } else {
                    (
                        right_rect,
                        &self.label_false,
                        selected_text_color,
                        &self.label_true,
                        text_color,
                    )
                };

            // Selected segment pill
            let inner_padding = 2.0;
            let sel_inner = sel_rect.shrink(inner_padding);
            ui.painter().rect(
                sel_inner,
                corner_radius,
                selected_bg,
                Stroke::NONE,
                StrokeKind::Middle,
            );

            // Draw labels
            let font_id = egui::FontId::proportional(self.height * 0.45);

            // Selected label
            let sel_galley =
                ui.painter()
                    .layout_no_wrap(sel_text.clone(), font_id.clone(), sel_text_color);
            let sel_pos = sel_rect.center() - sel_galley.rect.size() / 2.0;
            ui.painter()
                .galley(sel_pos, sel_galley, Color32::PLACEHOLDER);

            // Unselected label
            let unsel_rect = if *self.selected {
                right_rect
            } else {
                left_rect
            };
            let unsel_galley = ui.painter().layout_no_wrap(
                unsel_text.clone(),
                font_id,
                unsel_text_color.linear_multiply(0.7),
            );
            let unsel_pos = unsel_rect.center() - unsel_galley.rect.size() / 2.0;
            ui.painter()
                .galley(unsel_pos, unsel_galley, Color32::PLACEHOLDER);
        }

        #[cfg(debug_assertions)]
        crate::debug::paint_widget_rect_debug(
            ui.painter(),
            rect,
            "seg-ctrl",
            self.debug_lines || crate::debug::ui_debug_guidelines_enabled(),
        );

        response
    }
}
