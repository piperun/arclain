//! Theme definitions and the main AppTheme struct

use crate::ThemeColors;
use egui::{Context, Stroke};

/// The main application theme holder
#[derive(Clone)]
pub struct AppTheme {
    pub colors: ThemeColors,
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
        Self {
            colors: if dark_mode {
                ThemeColors::dark()
            } else {
                ThemeColors::light()
            },
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
        visuals.window_corner_radius = egui::CornerRadius::same(8);

        visuals.override_text_color = Some(self.colors.on_surface);

        ctx.set_visuals(visuals);
    }
}
