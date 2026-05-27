//! A themed slider widget with editable value display.
//! Y2K Monochrome styling: square thumb, sharp corners, editable text.

use arclain_theme::ThemeColors;
use egui::{CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget};

/// A slider with theme-aware styling and editable text label.
pub struct ThemedSlider<'a> {
    value: &'a mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: String,
    width: f32,
    height: f32,
    theme_colors: Option<&'a ThemeColors>,
    /// Internal state for text editing
    text_edit_id: Option<egui::Id>,
    debug_lines: bool,
}

impl<'a> ThemedSlider<'a> {
    pub fn new(value: &'a mut f32, range: std::ops::RangeInclusive<f32>) -> Self {
        Self {
            value,
            range,
            suffix: String::new(),
            width: 200.0,
            height: 28.0,
            theme_colors: None,
            text_edit_id: None,
            debug_lines: false,
        }
    }

    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
        self
    }

    /// Set a unique ID for the text edit (required for state persistence)
    pub fn id(mut self, id: egui::Id) -> Self {
        self.text_edit_id = Some(id);
        self
    }

    /// Force the debug overlay on. ORs with `EGUI_UI_DEBUG_GUIDELINES`.
    /// Stripped in release builds.
    pub fn debug_lines(mut self, on: bool) -> Self {
        self.debug_lines = on;
        self
    }
}

impl<'a> Widget for ThemedSlider<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let total_width = self.width;
        let value_width = 60.0;
        let slider_width = total_width - value_width - 8.0;
        let track_height = 4.0; // Thinner for Y2K look
        let thumb_size = 12.0;

        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(total_width, self.height), Sense::click_and_drag());

        // Handle drag on slider area
        let slider_rect = egui::Rect::from_min_size(rect.min, Vec2::new(slider_width, self.height));

        if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                if slider_rect.contains(pos) || response.drag_started() {
                    let relative_x = (pos.x - slider_rect.min.x).clamp(0.0, slider_width);
                    let t = relative_x / slider_width;
                    let min = *self.range.start();
                    let max = *self.range.end();
                    *self.value = min + t * (max - min);
                    response.mark_changed();
                }
            }
        }

        let colors = self.theme_colors;
        let visuals = ui.style().visuals.clone();

        // Y2K Colors from theme or fallback
        let track_bg = colors
            .map(|c| c.outline)
            .unwrap_or(visuals.widgets.inactive.bg_fill);
        let track_fill = colors
            .map(|c| c.primary)
            .unwrap_or(visuals.selection.bg_fill);
        let thumb_color = colors
            .map(|c| c.on_surface)
            .unwrap_or(visuals.strong_text_color());
        let text_color = colors.map(|c| c.on_surface).unwrap_or(visuals.text_color());

        if ui.is_rect_visible(rect) {
            // Calculate positions
            let min = *self.range.start();
            let max = *self.range.end();
            let t = (*self.value - min) / (max - min);

            let track_y = slider_rect.center().y;
            let track_rect = egui::Rect::from_center_size(
                egui::pos2(slider_rect.center().x, track_y),
                Vec2::new(slider_width - 16.0, track_height),
            );

            // Y2K: Zero radius for track
            let corner = CornerRadius::ZERO;

            // Draw track background
            ui.painter().rect(
                track_rect,
                corner,
                track_bg,
                Stroke::new(1.0, track_bg),
                StrokeKind::Middle,
            );

            // Draw filled portion
            let filled_width = track_rect.width() * t;
            if filled_width > 0.0 {
                let filled_rect = egui::Rect::from_min_size(
                    track_rect.min,
                    Vec2::new(filled_width, track_height),
                );
                ui.painter().rect(
                    filled_rect,
                    corner,
                    track_fill,
                    Stroke::NONE,
                    StrokeKind::Middle,
                );
            }

            // Y2K: Square thumb instead of circle
            let thumb_x = track_rect.min.x + filled_width;
            let thumb_center =
                egui::pos2(thumb_x.clamp(track_rect.min.x, track_rect.max.x), track_y);
            let thumb_rect = egui::Rect::from_center_size(thumb_center, Vec2::splat(thumb_size));

            // Thumb (square, Y2K style)
            ui.painter().rect(
                thumb_rect,
                CornerRadius::ZERO,
                thumb_color,
                Stroke::new(1.0, track_fill),
                StrokeKind::Middle,
            );
        }

        // Editable value text
        let value_rect = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - value_width, rect.min.y),
            Vec2::new(value_width, self.height),
        );

        // Create a child UI for the text edit
        let edit_id = self
            .text_edit_id
            .unwrap_or_else(|| ui.id().with("slider_edit"));

        // Get or create temporary text state
        let text_state_id = edit_id.with("text");
        let mut text_value = ui.data(|d| {
            d.get_temp::<String>(text_state_id)
                .unwrap_or_else(|| format!("{:.0}", *self.value))
        });

        // Check if we need to update from external value change
        let stored_value: f32 =
            ui.data(|d| d.get_temp(edit_id.with("stored")).unwrap_or(*self.value));
        if (stored_value - *self.value).abs() > 0.1 {
            text_value = format!("{:.0}", *self.value);
        }

        let text_edit_response = ui.put(
            value_rect.shrink2(Vec2::new(4.0, 4.0)),
            egui::TextEdit::singleline(&mut text_value)
                .font(egui::FontId::proportional(12.0))
                .horizontal_align(egui::Align::Center)
                .desired_width(value_width - 8.0)
                .text_color(text_color)
                .frame(true),
        );

        // Parse and apply on focus loss or enter
        if text_edit_response.lost_focus() {
            // Try to parse the value
            let parsed = text_value
                .trim()
                .trim_end_matches(&self.suffix)
                .trim()
                .parse::<f32>();

            if let Ok(new_val) = parsed {
                let clamped = new_val.clamp(*self.range.start(), *self.range.end());
                *self.value = clamped;
                text_value = format!("{:.0}", clamped);
                response.mark_changed();
            } else {
                // Reset to current value on parse failure
                text_value = format!("{:.0}", *self.value);
            }
        }

        // Store text state
        ui.data_mut(|d| {
            d.insert_temp(text_state_id, text_value);
            d.insert_temp(edit_id.with("stored"), *self.value);
        });

        #[cfg(debug_assertions)]
        crate::debug::paint_widget_rect_debug(
            ui.painter(),
            rect,
            "slider",
            self.debug_lines || crate::debug::ui_debug_guidelines_enabled(),
        );

        response
    }
}
