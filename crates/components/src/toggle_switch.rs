use egui::{Color32, CornerRadius, FontId, Response, Sense, Ui, Vec2, Widget};

/// A custom toggle switch component.
pub struct ToggleSwitch<'a> {
    on: &'a mut bool,
    text_on: String,
    text_off: String,
    color_on_bg: Option<Color32>,
    color_on_text: Option<Color32>,
    color_off_bg: Option<Color32>,
    color_off_text: Option<Color32>,
    width: f32,
    height: f32,
}

impl<'a> ToggleSwitch<'a> {
    pub fn new(on: &'a mut bool) -> Self {
        Self {
            on,
            text_on: "ON".to_string(),
            text_off: "OFF".to_string(),
            color_on_bg: None,    // Will default to ui.visuals().selection.bg_fill
            color_on_text: None,  // Will default to ui.visuals().selection.stroke.color
            color_off_bg: None,   // Will default to ui.visuals().faint_bg_color
            color_off_text: None, // Will default to ui.visuals().text_color() with opacity
            width: 40.0,
            height: 20.0,
        }
    }

    pub fn text(mut self, on: impl Into<String>, off: impl Into<String>) -> Self {
        self.text_on = on.into();
        self.text_off = off.into();
        self
    }

    /// Set styling for the ON state
    pub fn style_on(mut self, bg: Color32, text: Color32) -> Self {
        self.color_on_bg = Some(bg);
        self.color_on_text = Some(text);
        self
    }

    /// Set styling for the OFF state
    pub fn style_off(mut self, bg: Color32, text: Color32) -> Self {
        self.color_off_bg = Some(bg);
        self.color_off_text = Some(text);
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

            // Determine colors
            let (bg_color, text_color, text) = if *self.on {
                (
                    self.color_on_bg.unwrap_or(visuals.selection.bg_fill),
                    self.color_on_text.unwrap_or(visuals.strong_text_color()),
                    &self.text_on,
                )
            } else {
                (
                    self.color_off_bg.unwrap_or(visuals.extreme_bg_color),
                    self.color_off_text.unwrap_or(visuals.text_color()),
                    &self.text_off,
                )
            };

            // Draw background pill
            ui.painter().rect_filled(
                rect,
                CornerRadius::same((self.height / 2.0) as u8),
                bg_color,
            );
            // Draw text
            // Center the text
            let font_id = FontId::proportional(10.0);
            let galley = ui
                .painter()
                .layout_no_wrap(text.clone(), font_id, text_color);

            let text_pos = rect.center() - galley.rect.size() / 2.0;
            ui.painter().galley(text_pos, galley, Color32::PLACEHOLDER);
        }

        response
    }
}
