//! Theme definitions and the main AppTheme struct

use crate::{ThemeColors, ThemeExtensions};
use egui::{Context, Stroke};

/// The main application theme holder
#[derive(Clone)]
pub struct AppTheme {
    pub colors: ThemeColors,
    pub extensions: ThemeExtensions,
    pub dark_mode: bool,
}

impl Default for AppTheme {
    fn default() -> Self {
        Self::new(true) // Default to dark mode
    }
}

impl AppTheme {
    /// Create a new theme with the specified mode
    pub fn new(dark_mode: bool) -> Self {
        let colors = if dark_mode {
            ThemeColors::dark()
        } else {
            ThemeColors::light()
        };
        let extensions = ThemeExtensions::from_colors(&colors);
        Self {
            colors,
            extensions,
            dark_mode,
        }
    }

    /// Toggle between light and dark mode
    pub fn toggle(&mut self) {
        self.dark_mode = !self.dark_mode;
        self.colors = if self.dark_mode {
            ThemeColors::dark()
        } else {
            ThemeColors::light()
        };
        self.extensions = ThemeExtensions::from_colors(&self.colors);
    }

    /// Apply this theme to an egui Context
    pub fn apply_to_context(&self, ctx: &Context) {
        let mut visuals = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        // Apply semantic colors to egui visuals
        visuals.widgets.noninteractive.bg_fill = self.colors.surface;
        visuals.widgets.noninteractive.weak_bg_fill = self.colors.surface_variant;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.colors.outline);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.colors.on_surface);

        visuals.widgets.inactive.bg_fill = self.colors.surface_variant;
        visuals.widgets.inactive.weak_bg_fill = self.colors.surface_variant;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.colors.outline);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, self.colors.on_surface);

        visuals.widgets.hovered.bg_fill = self.colors.secondary;
        visuals.widgets.hovered.weak_bg_fill = self.colors.secondary;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, self.colors.outline);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, self.colors.on_surface);

        visuals.widgets.active.bg_fill = self.colors.primary;
        visuals.widgets.active.weak_bg_fill = self.colors.primary;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, self.colors.primary);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, self.colors.on_primary);

        visuals.selection.bg_fill = self.colors.selection;
        visuals.selection.stroke = Stroke::new(1.0, self.colors.primary);

        visuals.window_fill = self.colors.surface;
        visuals.panel_fill = self.colors.surface_variant;

        visuals.window_stroke = Stroke::new(1.0, self.colors.outline);
        visuals.window_corner_radius = egui::CornerRadius::ZERO; // Y2K: Razor sharp corners

        // Y2K: Zero radius for all widgets
        visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
        visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
        visuals.widgets.open.corner_radius = egui::CornerRadius::ZERO;

        visuals.override_text_color = Some(self.colors.on_surface);

        ctx.set_visuals(visuals);
    }
}
