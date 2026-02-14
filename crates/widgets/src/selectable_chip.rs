//! Selectable chip widget - an interactive chip with selected/active/inactive states

use arclain_theme::ThemeColors;
use egui::{Response, Ui};

/// An interactive chip with three visual states.
///
/// - **Selected**: Primary fill, strong border
/// - **Active**: Primary container fill, normal border
/// - **Inactive**: Dimmed surface, faint border
pub struct SelectableChip<'a> {
    text: &'a str,
    icon: Option<&'a str>,
    selected: bool,
    active: bool,
    colors: Option<&'a ThemeColors>,
}

impl<'a> SelectableChip<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            icon: None,
            selected: false,
            active: true,
            colors: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (bg, text_color, stroke) = if let Some(colors) = self.colors {
            if self.selected {
                (
                    colors.primary,
                    colors.on_primary,
                    egui::Stroke::new(2.0, colors.primary),
                )
            } else if self.active {
                (
                    colors.primary_container,
                    colors.on_primary_container,
                    egui::Stroke::new(1.0, colors.outline),
                )
            } else {
                (
                    colors.surface_variant.gamma_multiply(0.7),
                    colors.on_surface_variant,
                    egui::Stroke::new(1.0, colors.outline.gamma_multiply(0.5)),
                )
            }
        } else {
            let visuals = &ui.visuals().widgets.inactive;
            (
                visuals.bg_fill,
                visuals.fg_stroke.color,
                egui::Stroke::new(1.0, visuals.bg_stroke.color),
            )
        };

        let label = match self.icon {
            Some(icon) => format!("{} {}", icon, self.text),
            None => self.text.to_string(),
        };

        let chip = egui::Frame::NONE
            .fill(bg)
            .stroke(stroke)
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(&label).color(text_color));
            });

        ui.interact(
            chip.response.rect,
            chip.response.id,
            egui::Sense::click(),
        )
    }
}
