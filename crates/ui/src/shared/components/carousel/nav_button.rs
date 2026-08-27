//! Navigation button component for carousel

use crate::shared::theme::ThemeColors;
use eframe::egui;

/// Style configuration for nav buttons
#[derive(Clone)]
pub struct NavButtonStyle {
    pub width: f32,
    pub height: f32,
    pub icon_size: f32,
    pub corner_radius: u8,
}

impl Default for NavButtonStyle {
    fn default() -> Self {
        Self {
            width: 28.0,
            height: 64.0,
            icon_size: 14.0,
            corner_radius: 4,
        }
    }
}

impl NavButtonStyle {
    pub fn small() -> Self {
        Self {
            width: 24.0,
            height: 48.0,
            icon_size: 12.0,
            corner_radius: 4,
        }
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

/// Navigation button widget
pub struct NavButton<'a> {
    icon: &'a str,
    id_source: Option<&'a str>,
    style: NavButtonStyle,
    colors: Option<&'a ThemeColors>,
}

impl<'a> NavButton<'a> {
    pub fn new(icon: &'a str) -> Self {
        Self {
            icon,
            id_source: None,
            style: NavButtonStyle::default(),
            colors: None,
        }
    }

    /// Set a unique ID source for this button (to avoid ID clashes)
    pub fn id(mut self, id_source: &'a str) -> Self {
        self.id_source = Some(id_source);
        self
    }

    pub fn style(mut self, style: NavButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Show the button using normal layout (allocates space)
    pub fn show(self, ui: &mut egui::Ui) -> egui::Response {
        let size = egui::vec2(self.style.width, self.style.height);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        self.render(ui, rect, &response);
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// Show the button at a specific rect (doesn't allocate space)
    pub fn show_at(self, ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
        let id = match self.id_source {
            Some(source) => ui.id().with(source),
            None => ui
                .id()
                .with(self.icon)
                .with((rect.min.x as i32, rect.min.y as i32)),
        };
        let response = ui.interact(rect, id, egui::Sense::click());
        self.render(ui, rect, &response);
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn render(&self, ui: &egui::Ui, rect: egui::Rect, response: &egui::Response) {
        let fallback_colors = ThemeColors::dark();
        let colors = self.colors.unwrap_or(&fallback_colors);
        let is_hovered = response.hovered();

        // Background
        let bg_color = if is_hovered {
            colors.primary
        } else {
            colors.surface_variant
        };

        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(self.style.corner_radius),
            bg_color,
        );

        // Border
        let border_color = if is_hovered {
            colors.primary
        } else {
            colors.outline_variant
        };

        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(self.style.corner_radius),
            egui::Stroke::new(1.0_f32, border_color),
            egui::StrokeKind::Inside,
        );

        // Icon
        let icon_color = if is_hovered {
            colors.on_primary
        } else {
            colors.on_surface_variant
        };

        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            self.icon,
            egui::FontId::proportional(self.style.icon_size),
            icon_color,
        );
    }
}
