use crate::shared::theme::{AppTheme, ThemeColors};
use eframe::egui;

/// A standardized page layout for settings content
pub struct SettingsForm;

impl SettingsForm {
    pub fn new() -> Self {
        Self
    }

    pub fn show<F>(self, ui: &mut egui::Ui, _theme: &AppTheme, add_contents: F)
    where
        F: FnOnce(&mut egui::Ui),
    {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(24.0)) // Use inner margin for spacing
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("settings_form_scroll")
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        add_contents(ui);
                        ui.add_space(20.0);
                    });
            });
    }
}

/// A standardized section header for settings
pub struct SectionHeader {
    title: String,
}

impl SectionHeader {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    pub fn show(self, ui: &mut egui::Ui, colors: &ThemeColors) {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(self.title)
                .strong()
                .size(14.0)
                .color(colors.on_surface),
        );
        ui.add_space(4.0);
    }
}

/// A standardized row for a setting (Title + Description + Action)
pub struct SettingsRow<'a> {
    title: String,
    description: Option<String>,
    action: Box<dyn FnOnce(&mut egui::Ui) + 'a>,
}

impl<'a> SettingsRow<'a> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: None,
            action: Box::new(|_| {}),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn action(mut self, action: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.action = Box::new(action);
        self
    }

    pub fn show(self, ui: &mut egui::Ui, colors: &ThemeColors) {
        ui.allocate_ui(egui::vec2(ui.available_width(), 0.0), |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(self.title)
                            .strong()
                            .color(colors.on_surface),
                    );
                    if let Some(desc) = self.description {
                        ui.label(
                            egui::RichText::new(desc)
                                .small()
                                .color(colors.on_surface_variant),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    (self.action)(ui);
                });
            });
        });
        ui.add_space(8.0);
    }
}

/// A grouped container for related settings (Y2K boxed style)
/// Renders a bordered box with a title header and child content.
pub struct SettingsGroup<'a> {
    title: String,
    content: Box<dyn FnOnce(&mut egui::Ui, &ThemeColors) + 'a>,
}

impl<'a> SettingsGroup<'a> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: Box::new(|_, _| {}),
        }
    }

    /// Set the content to render inside the group
    pub fn content(mut self, content: impl FnOnce(&mut egui::Ui, &ThemeColors) + 'a) -> Self {
        self.content = Box::new(content);
        self
    }

    /// Y2K style: Sharp bordered box with header
    pub fn show(self, ui: &mut egui::Ui, colors: &ThemeColors) {
        ui.add_space(8.0);

        // Y2K: 1px border, zero radius
        egui::Frame::NONE
            .stroke(egui::Stroke::new(1.0, colors.outline))
            .inner_margin(egui::Margin::same(12))
            .corner_radius(egui::CornerRadius::ZERO)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Header row
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(13.0)
                            .color(colors.on_surface),
                    );
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Content
                (self.content)(ui, colors);
            });

        ui.add_space(8.0);
    }
}
