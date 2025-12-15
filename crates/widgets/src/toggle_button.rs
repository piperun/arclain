//! Toggle button widget - a button with selected/unselected state

use arclain_theme::ThemeColors;
use egui::{Response, Ui, Widget};

/// A button that can be toggled selected/unselected
pub struct ToggleButton<'a> {
    text: &'a str,
    selected: bool,
    size: egui::Vec2,
    colors: Option<&'a ThemeColors>,
}

impl<'a> ToggleButton<'a> {
    pub fn new(text: &'a str, selected: bool) -> Self {
        Self {
            text,
            selected,
            size: egui::vec2(36.0, 32.0),
            colors: None,
        }
    }

    pub fn size(mut self, size: egui::Vec2) -> Self {
        self.size = size;
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }
}

impl<'a> Widget for ToggleButton<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (bg_fill, text_color) = if let Some(colors) = self.colors {
            let bg = if self.selected {
                colors.secondary
            } else {
                egui::Color32::TRANSPARENT
            };
            (bg, colors.on_surface)
        } else {
            let bg = if self.selected {
                ui.visuals().widgets.active.bg_fill
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };
            (bg, ui.visuals().widgets.inactive.fg_stroke.color)
        };

        let button = egui::Button::new(egui::RichText::new(self.text).size(16.0).color(text_color))
            .fill(bg_fill)
            .stroke(egui::Stroke::NONE)
            .corner_radius(4.0)
            .min_size(self.size);

        ui.add(button)
    }
}
