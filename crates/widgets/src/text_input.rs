//! Text Input Widget
//!
//! A styled single-line text input with consistent height and padding.

use arclain_theme::ThemeColors;
use egui::{Response, TextEdit, Ui, Widget};

/// Height presets for text inputs
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TextInputSize {
    /// Small input: 28px height
    Small,
    /// Medium input: 32px height (default)
    #[default]
    Medium,
    /// Large input: 40px height
    Large,
}

impl TextInputSize {
    pub fn height(self) -> f32 {
        match self {
            TextInputSize::Small => 28.0,
            TextInputSize::Medium => 32.0,
            TextInputSize::Large => 40.0,
        }
    }

    pub fn font_size(self) -> f32 {
        match self {
            TextInputSize::Small => 12.0,
            TextInputSize::Medium => 13.0,
            TextInputSize::Large => 14.0,
        }
    }
}

/// A styled single-line text input
pub struct TextInput<'a> {
    text: &'a mut String,
    hint: Option<String>,
    size: TextInputSize,
    width: Option<f32>,
    theme_colors: Option<&'a ThemeColors>,
    monospace: bool,
}

impl<'a> TextInput<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            hint: None,
            size: TextInputSize::Medium,
            width: None,
            theme_colors: None,
            monospace: false,
        }
    }

    /// Set placeholder/hint text
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Set the input size
    pub fn size(mut self, size: TextInputSize) -> Self {
        self.size = size;
        self
    }

    /// Set a specific width
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Use monospace font
    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    /// Set theme colors
    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.theme_colors = Some(colors);
        self
    }
}

impl<'a> Widget for TextInput<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let height = self.size.height();

        // Get colors
        let (bg_fill, text_color, hint_color, stroke_color) = if let Some(colors) = self.theme_colors
        {
            (
                colors.surface_variant,
                colors.on_surface,
                colors.on_surface_variant,
                colors.outline_variant,
            )
        } else {
            let visuals = &ui.visuals().widgets.inactive;
            (
                visuals.bg_fill,
                visuals.fg_stroke.color,
                ui.visuals().weak_text_color(),
                visuals.bg_stroke.color,
            )
        };

        // Build the text edit with proper vertical alignment
        let mut edit = TextEdit::singleline(self.text)
            .vertical_align(egui::Align::Center)
            .min_size(egui::vec2(0.0, height))
            .text_color(text_color)
            .font(egui::TextStyle::Body)
            .frame(false); // We'll draw our own frame

        if let Some(hint) = &self.hint {
            edit = edit.hint_text(egui::RichText::new(hint).color(hint_color));
        }

        if let Some(width) = self.width {
            edit = edit.desired_width(width);
        }

        if self.monospace {
            edit = edit.font(egui::FontSelection::FontId(egui::FontId::monospace(13.0)));
        }

        // Draw custom frame with the text edit inside
        egui::Frame::new()
            .fill(bg_fill)
            .stroke(egui::Stroke::new(1.0, stroke_color))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 0))
            .show(ui, |ui| {
                ui.add(edit)
            })
            .inner
    }
}
