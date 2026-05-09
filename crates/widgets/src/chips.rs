//! Chip widget - a pill-shaped label/badge

use arclain_theme::ThemeColors;
use egui::{Response, Ui, Widget};

/// A pill-shaped chip/badge label.
///
/// Defaults to a passive label (hover-only response). Call
/// `.clickable(true)` to opt into click semantics — the returned
/// `Response` will then react to `clicked()` and the cursor will
/// switch to a hand pointer over the chip.
pub struct Chips<'a> {
    text: &'a str,
    colors: Option<&'a ThemeColors>,
    stroke_color: Option<egui::Color32>,
    background_color: Option<egui::Color32>,
    text_color: Option<egui::Color32>,
    clickable: bool,
}

impl<'a> Chips<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            colors: None,
            stroke_color: None,
            background_color: None,
            text_color: None,
            clickable: false,
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

    /// Override the background color
    pub fn background_color(mut self, color: egui::Color32) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Override the text color (defaults to `on_surface` from the
    /// theme or the inactive widget fg color).
    pub fn text_color(mut self, color: egui::Color32) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Make the chip clickable. The returned `Response` will report
    /// `clicked()`, hover state, and a hand-pointer cursor. Default
    /// is `false` (chip is a passive label).
    pub fn clickable(mut self, clickable: bool) -> Self {
        self.clickable = clickable;
        self
    }
}

impl<'a> Widget for Chips<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (bg_fill, stroke, text_color) = if let Some(colors) = self.colors {
            let stroke_col = self.stroke_color.unwrap_or(colors.outline);
            let bg = self.background_color.unwrap_or(colors.surface_variant);
            let txt = self.text_color.unwrap_or(colors.on_surface);
            (bg, egui::Stroke::new(1.0, stroke_col), txt)
        } else {
            let bg = self
                .background_color
                .unwrap_or(ui.visuals().widgets.inactive.bg_fill);
            let txt = self
                .text_color
                .unwrap_or(ui.visuals().widgets.inactive.fg_stroke.color);
            (
                bg,
                egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color),
                txt,
            )
        };

        let frame = egui::Frame::NONE
            .fill(bg_fill)
            .stroke(stroke)
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(10, 4));

        let inner = frame.show(ui, |ui| {
            // Use a non-selectable label inside so the chip area
            // doesn't show a text-cursor / drag-select feedback —
            // that read as "the text is floating" before this fix.
            ui.add(egui::Label::new(
                egui::RichText::new(self.text).size(12.0).color(text_color),
            ).selectable(false));
        });

        if self.clickable {
            // Upgrade the frame's hover-only response to a clickable
            // one over the same rect. `on_hover_cursor` flips the
            // pointer to a hand so users get the affordance.
            ui.interact(inner.response.rect, inner.response.id, egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            inner.response
        }
    }
}
