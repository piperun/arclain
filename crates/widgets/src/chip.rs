//! Chip widget - a pill-shaped label/badge

use arclain_theme::ThemeColors;
use egui::{Response, Ui, Widget};

/// A pill-shaped chip/badge label
pub struct Chip<'a> {
    text: &'a str,
    colors: Option<&'a ThemeColors>,
    stroke_color: Option<egui::Color32>,
}

impl<'a> Chip<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            colors: None,
            stroke_color: None,
        }
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Override the stroke/border color
    pub fn stroke_color(mut self, color: egui::Color32) -> Self {
        self.stroke_color = Some(color);
        self
    }
}

impl<'a> Widget for Chip<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (bg_fill, stroke, text_color) = if let Some(colors) = self.colors {
            let stroke_col = self.stroke_color.unwrap_or(colors.outline);
            (
                colors.surface_variant,
                egui::Stroke::new(1.0, stroke_col),
                colors.on_surface,
            )
        } else {
            (
                ui.visuals().widgets.inactive.bg_fill,
                egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color),
                ui.visuals().widgets.inactive.fg_stroke.color,
            )
        };

        let frame = egui::Frame::NONE
            .fill(bg_fill)
            .stroke(stroke)
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(10, 4));

        frame
            .show(ui, |ui| {
                ui.label(egui::RichText::new(self.text).size(12.0).color(text_color));
            })
            .response
    }
}
