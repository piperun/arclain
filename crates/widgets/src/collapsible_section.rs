//! Collapsible section widget with theme support

use arclain_theme::ThemeColors;
use egui::Ui;

/// A collapsible section with proper theme integration
pub struct CollapsibleSection<'a> {
    id: egui::Id,
    title: &'a str,
    default_open: bool,
    colors: Option<&'a ThemeColors>,
}

impl<'a> CollapsibleSection<'a> {
    pub fn new(id_source: impl std::hash::Hash, title: &'a str) -> Self {
        Self {
            id: egui::Id::new(id_source),
            title,
            default_open: true,
            colors: None,
        }
    }

    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Show the collapsible section with a body closure
    pub fn show<R>(self, ui: &mut Ui, add_body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let (text_color, icon_color) = if let Some(colors) = self.colors {
            (colors.on_surface, colors.on_surface_variant)
        } else {
            (ui.visuals().text_color(), ui.visuals().weak_text_color())
        };

        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            self.id,
            self.default_open,
        );

        // Render header
        let _header_res = ui.horizontal(|ui| {
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());

            if response.clicked() {
                state.toggle(ui);
            }

            if ui.is_rect_visible(rect) {
                // Draw triangle icon
                let icon = if state.is_open() { "▼" } else { "▶" };
                let icon_pos = egui::pos2(rect.min.x + 4.0, rect.center().y);
                ui.painter().text(
                    icon_pos,
                    egui::Align2::LEFT_CENTER,
                    icon,
                    egui::FontId::proportional(10.0),
                    icon_color,
                );

                // Draw title
                let title_pos = egui::pos2(rect.min.x + 18.0, rect.center().y);
                ui.painter().text(
                    title_pos,
                    egui::Align2::LEFT_CENTER,
                    self.title,
                    egui::FontId::proportional(11.0),
                    text_color,
                );
            }
        });

        // Store state
        state.store(ui.ctx());

        // Render body if open
        if state.is_open() {
            // Set text color for body content
            if let Some(colors) = self.colors {
                ui.visuals_mut().override_text_color = Some(colors.on_surface);
            }
            Some(add_body(ui))
        } else {
            None
        }
    }
}
