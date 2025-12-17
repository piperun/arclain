use egui::{
    lerp, pos2, Color32, CornerRadius, FontId, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    Widget,
};

/// A custom fancy toggle switch component with animations and icons.
pub struct ToggleSwitch<'a> {
    on: &'a mut bool,
    text_on: Option<String>,
    text_off: Option<String>,
    icon_on: Option<String>,
    icon_off: Option<String>,
    width: f32,
    height: f32,
}

impl<'a> ToggleSwitch<'a> {
    pub fn new(on: &'a mut bool) -> Self {
        Self {
            on,
            text_on: None,
            text_off: None,
            icon_on: None,
            icon_off: None,
            width: 44.0,
            height: 22.0,
        }
    }

    pub fn text(mut self, on: impl Into<String>, off: impl Into<String>) -> Self {
        self.text_on = Some(on.into());
        self.text_off = Some(off.into());
        self
    }

    pub fn icons(mut self, on: impl Into<String>, off: impl Into<String>) -> Self {
        self.icon_on = Some(on.into());
        self.icon_off = Some(off.into());
        self
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

impl<'a> Widget for ToggleSwitch<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(self.width, self.height), Sense::click());

        if response.clicked() {
            *self.on = !*self.on;
            response.mark_changed();
        }

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().visuals.clone();

            // Animation state
            let how_on = ui.ctx().animate_bool(response.id, *self.on);

            // layout
            let thumb_radius = (self.height / 2.0) - 2.0;
            let padding = 2.0;

            // Background Color
            let color_off = visuals.widgets.inactive.bg_fill;
            let color_on = visuals.selection.bg_fill;
            let bg_color = Color32::from_rgb(
                lerp(color_off.r() as f32..=color_on.r() as f32, how_on) as u8,
                lerp(color_off.g() as f32..=color_on.g() as f32, how_on) as u8,
                lerp(color_off.b() as f32..=color_on.b() as f32, how_on) as u8,
            );

            // Paint Background Pill
            let radius = (self.height / 2.0) as u8;
            let corner_radius = CornerRadius::same(radius);

            // Use painter().rect with 5 arguments: rect, radius, fill, stroke, stroke_kind
            let stroke = Stroke::new(1.0, bg_color.linear_multiply(1.2));
            ui.painter()
                .rect(rect, corner_radius, bg_color, stroke, StrokeKind::Middle);

            // Thumb Position
            let min_x = rect.min.x + padding + thumb_radius;
            let max_x = rect.max.x - padding - thumb_radius;
            let thumb_center_x = lerp(min_x..=max_x, how_on);
            let thumb_center = pos2(thumb_center_x, rect.center().y);

            // Thumb Color
            let thumb_color = visuals.strong_text_color();

            // Glow Effect
            if how_on > 0.0 {
                let glow_alpha = (how_on * 50.0) as u8;
                let glow_color = Color32::from_rgba_premultiplied(
                    color_on.r(),
                    color_on.g(),
                    color_on.b(),
                    glow_alpha,
                );
                ui.painter()
                    .circle_filled(thumb_center, thumb_radius + 4.0, glow_color);
            }

            // Paint Thumb
            ui.painter()
                .circle_filled(thumb_center, thumb_radius, thumb_color);

            // Icon/Text
            let icon_str = if *self.on {
                self.icon_on
                    .as_deref()
                    .or(self.text_on.as_deref())
                    .unwrap_or("")
            } else {
                self.icon_off
                    .as_deref()
                    .or(self.text_off.as_deref())
                    .unwrap_or("")
            };

            if !icon_str.is_empty() {
                let font_id = FontId::proportional(thumb_radius * 1.3);

                let icon_r = lerp(color_off.r() as f32..=color_on.r() as f32, how_on) as u8;
                let icon_g = lerp(color_off.g() as f32..=color_on.g() as f32, how_on) as u8;
                let icon_b = lerp(color_off.b() as f32..=color_on.b() as f32, how_on) as u8;
                let dynamic_icon_color = Color32::from_rgb(icon_r, icon_g, icon_b);

                let galley =
                    ui.painter()
                        .layout_no_wrap(icon_str.to_string(), font_id, dynamic_icon_color);
                let text_pos = thumb_center - galley.rect.size() / 2.0;
                ui.painter().galley(text_pos, galley, Color32::PLACEHOLDER);
            }
        }

        response
    }
}
