#![allow(unused_imports)]
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render a standardized settings page header
pub struct SettingsHeader<'a> {
    pub icon: String,
    pub title: String,
    pub description: Option<String>,
    pub has_changes: bool,
    pub on_save: Option<Box<dyn FnOnce() + 'a>>,
    pub on_back: Option<Box<dyn FnOnce() + 'a>>,
    pub custom_actions: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    pub secondary_row: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    pub tertiary_row: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
}

impl<'a> SettingsHeader<'a> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            icon: egui_phosphor::regular::PLUGS.to_string(),
            title: title.into(),
            description: None,
            has_changes: false,
            on_save: None,
            on_back: None,
            custom_actions: None,
            secondary_row: None,
            tertiary_row: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        let d = description.into();
        if !d.is_empty() {
            self.description = Some(d);
        } else {
            self.description = None;
        }
        self
    }

    pub fn has_changes(mut self, has_changes: bool) -> Self {
        self.has_changes = has_changes;
        self
    }

    pub fn on_save(mut self, action: impl FnOnce() + 'a) -> Self {
        self.on_save = Some(Box::new(action));
        self
    }

    pub fn on_back(mut self, action: impl FnOnce() + 'a) -> Self {
        self.on_back = Some(Box::new(action));
        self
    }

    pub fn custom_actions(mut self, actions: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.custom_actions = Some(Box::new(actions));
        self
    }

    pub fn secondary_row(mut self, row: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.secondary_row = Some(Box::new(row));
        self
    }

    pub fn tertiary_row(mut self, row: impl FnOnce(&mut egui::Ui) + 'a) -> Self {
        self.tertiary_row = Some(Box::new(row));
        self
    }

    pub fn show(self, ui: &mut egui::Ui, theme: &AppTheme) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

            // Back Button (if present)
            if let Some(on_back) = self.on_back {
                if ui
                    .add(
                        arclain_widgets::IconButton::new("⬅")
                            .size(arclain_widgets::icon_button::IconButtonSize::Small),
                    )
                    .on_hover_text("Go Back")
                    .clicked()
                {
                    on_back();
                }
            }

            // Icon
            ui.label(egui::RichText::new(self.icon).size(24.0));

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);

                ui.label(
                    egui::RichText::new(self.title)
                        .size(20.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );

                if let Some(description) = self.description {
                    ui.label(
                        egui::RichText::new(description)
                            .size(12.0)
                            .color(theme.colors.on_surface_variant),
                    );
                }
            });

            // Right side (Save button, Custom actions in Main Row)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(20.0);

                // Save Button
                if let Some(on_save) = self.on_save {
                    let save_btn = egui::Button::new(
                        egui::RichText::new(format!(
                            "{} Save",
                            egui_phosphor::regular::FLOPPY_DISK
                        ))
                        .size(14.0)
                        .color(if self.has_changes {
                            theme.colors.on_primary
                        } else {
                            theme.colors.on_surface_variant
                        }),
                    )
                    .fill(if self.has_changes {
                        theme.colors.primary
                    } else {
                        theme.colors.secondary
                    })
                    .stroke(if self.has_changes {
                        egui::Stroke::NONE
                    } else {
                        egui::Stroke::new(1.0, theme.colors.outline)
                    })
                    .corner_radius(6.0)
                    .min_size(egui::vec2(90.0, 32.0));

                    if ui.add_enabled(self.has_changes, save_btn).clicked() {
                        on_save();
                    }
                    ui.add_space(8.0);
                }

                // Custom actions (Main Row)
                if let Some(actions) = self.custom_actions {
                    actions(ui);
                }
            });
        });

        // Secondary Row
        if let Some(row) = self.secondary_row {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                row(ui);
            });
        }

        // Tertiary Row
        if let Some(row) = self.tertiary_row {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                row(ui);
            });
        }

        ui.add_space(8.0);
        ui.separator();
    }
}
