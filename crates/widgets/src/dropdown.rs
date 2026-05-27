//! Themed dropdown widget - a styled wrapper around egui::ComboBox

use arclain_theme::ThemeColors;
use egui::{InnerResponse, Ui};

/// A themed dropdown (ComboBox wrapper) with consistent styling.
pub struct ThemedDropdown<'a> {
    id: &'a str,
    selected_text: String,
    colors: Option<&'a ThemeColors>,
    width: Option<f32>,
    debug_lines: bool,
}

impl<'a> ThemedDropdown<'a> {
    pub fn new(id: &'a str, selected_text: impl Into<String>) -> Self {
        Self {
            id,
            selected_text: selected_text.into(),
            colors: None,
            width: None,
            debug_lines: false,
        }
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Force the debug overlay on for the closed combo button rect.
    /// ORs with `EGUI_UI_DEBUG_GUIDELINES`. Stripped in release builds.
    pub fn debug_lines(mut self, on: bool) -> Self {
        self.debug_lines = on;
        self
    }

    /// Show the dropdown with a closure that populates options.
    pub fn show_ui<R>(
        self,
        ui: &mut Ui,
        menu_contents: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<Option<R>> {
        let text = if let Some(colors) = self.colors {
            egui::RichText::new(&self.selected_text).color(colors.on_surface)
        } else {
            egui::RichText::new(&self.selected_text)
        };

        let mut combo = egui::ComboBox::from_id_salt(self.id).selected_text(text);

        if let Some(w) = self.width {
            combo = combo.width(w);
        }

        let inner = combo.show_ui(ui, menu_contents);

        #[cfg(debug_assertions)]
        crate::debug::paint_widget_rect_debug(
            ui.painter(),
            inner.response.rect,
            "dropdown",
            self.debug_lines || crate::debug::ui_debug_guidelines_enabled(),
        );

        inner
    }
}
