//! SettingsCard - clickable card for settings navigation

use arclain_theme::ThemeColors;
use eframe::egui;

/// A clickable settings card with icon, title, and description.
pub struct SettingsCard<'a> {
    icon: &'a str,
    title: &'a str,
    description: &'a str,
    width: f32,
    height: f32,
    colors: &'a ThemeColors,
}

impl<'a> SettingsCard<'a> {
    pub fn new(
        icon: &'a str,
        title: &'a str,
        description: &'a str,
        colors: &'a ThemeColors,
    ) -> Self {
        Self {
            icon,
            title,
            description,
            width: 280.0,
            height: 100.0,
            colors,
        }
    }

    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Show the card. Returns true if clicked.
    pub fn show(self, ui: &mut egui::Ui) -> bool {
        let colors = self.colors;

        let card = egui::Frame::NONE
            .fill(colors.surface_variant)
            .stroke(egui::Stroke::new(1.0, colors.outline))
            .corner_radius(8.0)
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.set_width(self.width);
                ui.set_height(self.height);

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);

                    ui.label(egui::RichText::new(self.icon).size(24.0));

                    ui.label(
                        egui::RichText::new(self.title)
                            .size(16.0)
                            .strong()
                            .color(colors.on_surface),
                    );

                    ui.label(
                        egui::RichText::new(self.description)
                            .size(12.0)
                            .color(colors.on_surface_variant),
                    );
                });
            })
            .response;

        let clicked = card.interact(egui::Sense::click()).clicked();

        if card.hovered() {
            ui.painter()
                .rect_filled(card.rect, 8.0, colors.primary.linear_multiply(0.1));
        }

        clicked
    }

    /// Show a compact card variant (icon + title horizontal). Returns true if clicked.
    pub fn show_compact(self, ui: &mut egui::Ui) -> bool {
        let colors = self.colors;

        let card = egui::Frame::NONE
            .fill(colors.surface_variant)
            .stroke(egui::Stroke::new(1.0, colors.outline))
            .corner_radius(8.0)
            .inner_margin(20.0)
            .show(ui, |ui| {
                ui.set_width(self.width);
                ui.set_height(self.height);

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(self.icon).size(20.0));
                        ui.label(
                            egui::RichText::new(self.title)
                                .size(15.0)
                                .strong()
                                .color(colors.on_surface),
                        );
                    });
                    ui.label(
                        egui::RichText::new(self.description)
                            .size(12.0)
                            .color(colors.on_surface_variant),
                    );
                });
            })
            .response;

        let clicked = card.interact(egui::Sense::click()).clicked();

        if card.hovered() {
            ui.painter()
                .rect_filled(card.rect, 8.0, colors.primary.linear_multiply(0.1));
        }

        clicked
    }
}
