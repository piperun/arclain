use crate::shared::theme::{AppTheme, ThemeColors};
use eframe::egui;

/// A standardized page layout for content with scrolling
pub struct Form {
    id: Option<String>,
    margin: f32,
}

impl Form {
    pub fn new() -> Self {
        Self {
            id: None,
            margin: 24.0,
        }
    }

    /// Set a custom scroll area ID (for multiple forms on same page)
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set custom inner margin (default: 24.0)
    pub fn margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }

    pub fn show<F>(self, ui: &mut egui::Ui, _theme: &AppTheme, add_contents: F)
    where
        F: FnOnce(&mut egui::Ui),
    {
        let scroll_id = self.id.unwrap_or_else(|| "form_scroll".to_string());

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(self.margin))
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(scroll_id)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        add_contents(ui);
                        ui.add_space(20.0);
                    });
            });
    }
}

/// A standardized section header for settings with optional level hierarchy (h1-h4)
pub struct SectionHeader {
    title: String,
    level: u32,
    description: Option<String>,
}

impl SectionHeader {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            level: 3, // Default matches previous behavior (14px bold)
            description: None,
        }
    }

    /// Set the heading level (1=largest h1, 2=h2, 3=h3, 4+=smallest)
    pub fn level(mut self, level: u32) -> Self {
        self.level = level;
        self
    }

    /// Add a description/subtitle below the title
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn show(self, ui: &mut egui::Ui, colors: &ThemeColors) {
        let (font_size, is_bold) = match self.level {
            1 => (20.0, true),
            2 => (16.0, true),
            3 => (14.0, true),
            _ => (13.0, false),
        };

        ui.add_space(8.0);

        let mut title_text = egui::RichText::new(&self.title)
            .size(font_size)
            .color(colors.on_surface);
        if is_bold {
            title_text = title_text.strong();
        }
        ui.label(title_text);

        if let Some(desc) = &self.description {
            ui.label(
                egui::RichText::new(desc)
                    .size(12.0)
                    .color(colors.on_surface_variant),
            );
        }

        // Add separator for h1/h2 levels
        if self.level <= 2 {
            ui.add_space(4.0);
            ui.separator();
        } else {
            ui.add_space(4.0);
        }
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
#[allow(dead_code)]
pub struct SettingsGroup<'a> {
    title: String,
    content: Box<dyn FnOnce(&mut egui::Ui, &ThemeColors) + 'a>,
}

#[allow(dead_code)]
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

