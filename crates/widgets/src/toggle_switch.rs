use arclain_theme::ThemeColors;
use egui::{
    lerp, pos2, Color32, CornerRadius, FontId, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    Widget,
};

/// A custom fancy toggle switch component with animations and icons.
/// Uses theme colors for consistent styling.
pub struct ToggleSwitch<'a> {
    on: &'a mut bool,
    text_on: Option<String>,
    text_off: Option<String>,
    icon_on: Option<String>,
    icon_off: Option<String>,
    width: f32,
    height: f32,
    theme_colors: Option<&'a ThemeColors>,
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
            theme_colors: None,
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

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
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
            // Animation state
            let how_on = ui.ctx().animate_bool(response.id, *self.on);

            // Layout
            let thumb_radius = (self.height / 2.0) - 2.0;
            let padding = 2.0;

            // Get colors from theme or fallback to visuals
            let visuals = ui.style().visuals.clone();

            // Y2K Colors from theme:
            // OFF: black track, grey ball, subtle ring
            // ON: grey track, white ball, medium grey ring (visible glow)
            let (track_off, track_on, ball_off, ball_on, ring_off, ring_on) =
                if let Some(colors) = self.theme_colors {
                    (
                        colors.surface_variant,    // track when off (black #000)
                        colors.outline,            // track when on (grey #333)
                        colors.outline,            // ball when off (grey #333, visible!)
                        colors.primary,            // ball when on (white #FFF)
                        colors.outline_variant,    // ring when off (subtle #1A1A1A)
                        Color32::from_rgb(100, 100, 100), // ring when on (medium grey, visible)
                    )
                } else if visuals.dark_mode {
                    // Y2K Dark mode: OFF = black ball, ON = white ball
                    (
                        Color32::from_rgb(24, 24, 24),    // #181818 track off
                        Color32::from_rgb(64, 64, 64),    // #404040 track on
                        Color32::from_rgb(0, 0, 0),       // BLACK ball off
                        Color32::from_rgb(255, 255, 255), // WHITE ball on
                        Color32::from_rgb(40, 40, 40),    // dark ring off
                        Color32::from_rgb(180, 180, 180), // light ring on
                    )
                } else {
                    // Y2K Light mode: inverted
                    (
                        Color32::from_rgb(220, 220, 220), // light track off
                        Color32::from_rgb(160, 160, 160), // darker track on
                        Color32::from_rgb(255, 255, 255), // WHITE ball off
                        Color32::from_rgb(0, 0, 0),       // BLACK ball on
                        Color32::from_rgb(200, 200, 200), // light ring off
                        Color32::from_rgb(60, 60, 60),    // dark ring on
                    )
                };

            // Interpolate track color
            let bg_color = Color32::from_rgb(
                lerp(track_off.r() as f32..=track_on.r() as f32, how_on) as u8,
                lerp(track_off.g() as f32..=track_on.g() as f32, how_on) as u8,
                lerp(track_off.b() as f32..=track_on.b() as f32, how_on) as u8,
            );

            // Interpolate ball color
            let thumb_color = Color32::from_rgb(
                lerp(ball_off.r() as f32..=ball_on.r() as f32, how_on) as u8,
                lerp(ball_off.g() as f32..=ball_on.g() as f32, how_on) as u8,
                lerp(ball_off.b() as f32..=ball_on.b() as f32, how_on) as u8,
            );

            // Interpolate ring color (animated)
            let ring_color = Color32::from_rgb(
                lerp(ring_off.r() as f32..=ring_on.r() as f32, how_on) as u8,
                lerp(ring_off.g() as f32..=ring_on.g() as f32, how_on) as u8,
                lerp(ring_off.b() as f32..=ring_on.b() as f32, how_on) as u8,
            );

            // Paint Background Pill (rounded for toggle)
            let radius = (self.height / 2.0) as u8;
            let corner_radius = CornerRadius::same(radius);
            let border_color = self
                .theme_colors
                .map(|c| c.outline)
                .unwrap_or(visuals.widgets.inactive.bg_stroke.color);
            let stroke = Stroke::new(1.0, border_color);
            ui.painter()
                .rect(rect, corner_radius, bg_color, stroke, StrokeKind::Middle);

            // Thumb Position
            let min_x = rect.min.x + padding + thumb_radius;
            let max_x = rect.max.x - padding - thumb_radius;
            let thumb_center_x = lerp(min_x..=max_x, how_on);
            let thumb_center = pos2(thumb_center_x, rect.center().y);

            // Ring/glow around ball (+4px, animated color)
            ui.painter()
                .circle_filled(thumb_center, thumb_radius + 4.0, ring_color);

            // Paint Thumb (ball)
            ui.painter()
                .circle_filled(thumb_center, thumb_radius, thumb_color);

            // Icon/Text on the ball
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
                // Icon color is opposite of ball for contrast
                let icon_color = if let Some(colors) = self.theme_colors {
                    if how_on > 0.5 {
                        colors.on_primary
                    } else {
                        colors.on_surface_variant
                    }
                } else {
                    if how_on > 0.5 {
                        Color32::BLACK
                    } else {
                        Color32::from_rgb(80, 80, 80)
                    }
                };

                let galley = ui
                    .painter()
                    .layout_no_wrap(icon_str.to_string(), font_id, icon_color);
                let text_pos = thumb_center - galley.rect.size() / 2.0;
                ui.painter().galley(text_pos, galley, Color32::PLACEHOLDER);
            }
        }

        response
    }
}
