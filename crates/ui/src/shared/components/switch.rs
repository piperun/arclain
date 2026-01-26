//! Toggle Switch Component
//!
//! A Material-style toggle switch for boolean values.

use eframe::egui::{self, Response, Ui, Widget};

/// A toggle switch widget (iOS/Material style)
pub struct Switch<'a> {
    value: &'a mut bool,
    size: SwitchSize,
}

#[derive(Clone, Copy, Default)]
pub enum SwitchSize {
    Small,
    #[default]
    Medium,
}

impl SwitchSize {
    fn dimensions(&self) -> (f32, f32) {
        match self {
            SwitchSize::Small => (32.0, 18.0),
            SwitchSize::Medium => (40.0, 22.0),
        }
    }
}

impl<'a> Switch<'a> {
    pub fn new(value: &'a mut bool) -> Self {
        Self {
            value,
            size: SwitchSize::default(),
        }
    }

    pub fn small(mut self) -> Self {
        self.size = SwitchSize::Small;
        self
    }
}

impl<'a> Widget for Switch<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (width, height) = self.size.dimensions();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(width, height),
            egui::Sense::click(),
        );

        if response.clicked() {
            *self.value = !*self.value;
        }

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);
            let radius = height / 2.0;

            // Track background
            let bg_color = if *self.value {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };

            ui.painter().rect_filled(rect, radius, bg_color);

            // Thumb (the circle that slides)
            let thumb_radius = radius - 3.0;
            let thumb_x = if *self.value {
                rect.right() - radius
            } else {
                rect.left() + radius
            };
            let thumb_center = egui::pos2(thumb_x, rect.center().y);

            ui.painter().circle_filled(
                thumb_center,
                thumb_radius,
                visuals.fg_stroke.color,
            );
        }

        response
    }
}
