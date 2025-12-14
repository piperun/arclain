//! Icon-only button widget

use arclain_theme::ThemeColors;
use egui::{Response, Ui, Widget};

/// Size presets for icon buttons
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum IconButtonSize {
    /// Small: 28x28
    Small,
    /// Medium: 32x32 (default)
    #[default]
    Medium,
    /// Large: 40x40
    Large,
}

impl IconButtonSize {
    pub fn to_size(self) -> f32 {
        match self {
            IconButtonSize::Small => 28.0,
            IconButtonSize::Medium => 32.0,
            IconButtonSize::Large => 40.0,
        }
    }

    pub fn icon_size(self) -> f32 {
        match self {
            IconButtonSize::Small => 14.0,
            IconButtonSize::Medium => 16.0,
            IconButtonSize::Large => 20.0,
        }
    }
}

/// An icon-only button with theme-aware colors
pub struct IconButton<'a> {
    icon: &'a str,
    size: IconButtonSize,
    enabled: bool,
    colors: Option<&'a ThemeColors>,
}

impl<'a> IconButton<'a> {
    pub fn new(icon: &'a str) -> Self {
        Self {
            icon,
            size: IconButtonSize::Medium,
            enabled: true,
            colors: None,
        }
    }

    pub fn size(mut self, size: IconButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }
}

impl<'a> Widget for IconButton<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (bg_fill, text_color) = if let Some(colors) = self.colors {
            let text = if self.enabled {
                colors.on_surface
            } else {
                colors.on_surface_variant
            };
            (colors.surface_variant, text)
        } else {
            (
                ui.visuals().widgets.inactive.bg_fill,
                ui.visuals().widgets.inactive.fg_stroke.color,
            )
        };

        let size = self.size.to_size();
        let icon_size = self.size.icon_size();

        let button = egui::Button::new(
            egui::RichText::new(self.icon)
                .size(icon_size)
                .color(text_color),
        )
        .fill(bg_fill)
        .stroke(egui::Stroke::NONE)
        .corner_radius(4.0)
        .min_size(egui::vec2(size, size));

        ui.add_enabled(self.enabled, button)
    }
}
