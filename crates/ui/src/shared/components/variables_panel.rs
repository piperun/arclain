//! Variable Picker Dialog
//!
//! A dialog for selecting template variables with search and tabbed categories.

use super::SearchBar;
use crate::shared::theme::AppTheme;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

/// A single variable definition
#[derive(Clone, Debug)]
pub struct TemplateVariable {
    /// Variable name (e.g., "name", "date")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Example value for preview
    pub example: Option<String>,
}

impl TemplateVariable {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            example: None,
        }
    }

    pub fn with_example(mut self, example: impl Into<String>) -> Self {
        self.example = Some(example.into());
        self
    }

    /// Get the template placeholder format
    pub fn placeholder(&self) -> String {
        format!("{{{}}}", self.name)
    }

    /// Check if variable matches search query
    pub fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        let query = query.trim_start_matches('$').trim_start_matches('{');
        self.name.to_lowercase().contains(&query)
            || self.description.to_lowercase().contains(&query)
    }
}

/// A group of variables from a single source
#[derive(Clone, Debug)]
pub struct VariableGroup {
    /// Group name (e.g., "General", "DLSite")
    pub name: String,
    /// Short identifier for tab
    pub id: String,
    /// Variables in this group
    pub variables: Vec<TemplateVariable>,
}

impl VariableGroup {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let id = name.to_lowercase().replace(' ', "_");
        Self {
            name,
            id,
            variables: Vec::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_variables(mut self, variables: Vec<TemplateVariable>) -> Self {
        self.variables = variables;
        self
    }
}

/// Dialog for picking template variables
pub struct VariablePicker {
    /// All variable groups
    groups: Vec<VariableGroup>,
    /// Search query
    search: String,
    /// Currently selected tab index
    selected_tab: usize,
    /// Whether dialog is open
    open: bool,
}

impl Default for VariablePicker {
    fn default() -> Self {
        Self {
            groups: Self::builtin_variables(),
            search: String::new(),
            selected_tab: 0,
            open: false,
        }
    }
}

impl VariablePicker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a variable group (e.g., from a plugin)
    pub fn add_group(&mut self, group: VariableGroup) {
        self.groups.push(group);
    }

    /// Open the picker dialog
    pub fn open(&mut self) {
        self.open = true;
        self.search.clear();
    }

    /// Close the picker dialog
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Check if dialog is open
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Get built-in variables
    pub fn builtin_variables() -> Vec<VariableGroup> {
        vec![VariableGroup::new("General")
            .with_id("general")
            .with_variables(vec![
                TemplateVariable::new("name", "Original archive filename (without extension)")
                    .with_example("MyArchive"),
                TemplateVariable::new("ext", "Original file extension").with_example("zip"),
                TemplateVariable::new("date", "Current date (YYYY-MM-DD)")
                    .with_example("2024-01-15"),
                TemplateVariable::new("time", "Current time (HH-MM-SS)").with_example("14-30-00"),
                TemplateVariable::new("timestamp", "Unix timestamp").with_example("1705329000"),
            ])]
    }

    /// Show the picker dialog, returns selected variable placeholder if any
    pub fn show(&mut self, ctx: &egui::Context, theme: &AppTheme) -> Option<String> {
        if !self.open {
            return None;
        }

        let mut selected: Option<String> = None;

        egui::Window::new("Insert Variable")
            .collapsible(false)
            .resizable(false)
            .fixed_size([360.0, 320.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui_phosphor::regular::MAGNIFYING_GLASS);
                    let avail = ui.available_width() - 8.0;
                    let search_response = ui.add(
                        SearchBar::new(&mut self.search)
                            .hint("Search variables (e.g. $title)")
                            .width(avail)
                            .with_theme_colors(&theme.colors),
                    );
                    // Focus search on open
                    if search_response.gained_focus() || self.search.is_empty() {
                        search_response.request_focus();
                    }
                });

                ui.add_space(8.0);

                let has_search = !self.search.trim().is_empty();

                if has_search {
                    // Show filtered results across all groups
                    ui.separator();
                    ui.add_space(4.0);

                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            let mut found_any = false;
                            for group in &self.groups {
                                let matches: Vec<_> = group
                                    .variables
                                    .iter()
                                    .filter(|v| v.matches(&self.search))
                                    .collect();

                                if !matches.is_empty() {
                                    found_any = true;
                                    ui.label(
                                        egui::RichText::new(&group.name)
                                            .size(11.0)
                                            .color(theme.colors.on_surface_variant),
                                    );
                                    ui.add_space(4.0);

                                    for var in matches {
                                        if Self::render_variable_row(ui, theme, var) {
                                            selected = Some(var.placeholder());
                                        }
                                    }
                                    ui.add_space(8.0);
                                }
                            }

                            if !found_any {
                                ui.label(
                                    egui::RichText::new("No matching variables")
                                        .color(theme.colors.on_surface_variant),
                                );
                            }
                        });
                } else {
                    // Show tabs for groups
                    ui.horizontal(|ui| {
                        for (idx, group) in self.groups.iter().enumerate() {
                            let is_selected = self.selected_tab == idx;
                            let text = egui::RichText::new(&group.name).size(12.0);

                            let button = if is_selected {
                                egui::Button::new(text.strong())
                                    .fill(theme.colors.primary)
                                    .stroke(egui::Stroke::NONE)
                            } else {
                                egui::Button::new(text)
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        theme.colors.outline_variant,
                                    ))
                            };

                            if ui.add(button).clicked() {
                                self.selected_tab = idx;
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // Show variables for selected tab
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            if let Some(group) = self.groups.get(self.selected_tab) {
                                for var in &group.variables {
                                    if Self::render_variable_row(ui, theme, var) {
                                        selected = Some(var.placeholder());
                                    }
                                }
                            }
                        });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Cancel button
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                TextButton::new("Cancel", ButtonSize::Small)
                                    .with_theme_colors(&theme.colors),
                            )
                            .clicked()
                        {
                            self.open = false;
                        }
                    });
                });
            });

        // Close dialog if variable was selected
        if selected.is_some() {
            self.open = false;
        }

        selected
    }

    /// Render a single variable row, returns true if clicked
    fn render_variable_row(ui: &mut egui::Ui, theme: &AppTheme, var: &TemplateVariable) -> bool {
        let row_height = 28.0;
        let available_width = ui.available_width();

        // Allocate space and create interactive response FIRST
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(available_width, row_height),
            egui::Sense::click(),
        );

        // Draw hover/selection background
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, theme.colors.surface_variant);
        }

        // Draw content on top
        let text_rect = rect.shrink2(egui::vec2(8.0, 4.0));

        // Variable placeholder (left side)
        let placeholder_text = var.placeholder();
        let placeholder_galley = ui.painter().layout_no_wrap(
            placeholder_text.clone(),
            egui::FontId::new(12.0, egui::FontFamily::Monospace),
            theme.colors.primary,
        );
        ui.painter().galley(
            egui::pos2(
                text_rect.left(),
                text_rect.center().y - placeholder_galley.size().y / 2.0,
            ),
            placeholder_galley,
            theme.colors.primary,
        );

        // Description (right side, after placeholder)
        let desc = if var.description.len() > 40 {
            format!("{}...", &var.description[..37])
        } else {
            var.description.clone()
        };
        let desc_galley = ui.painter().layout_no_wrap(
            desc,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
            theme.colors.on_surface_variant,
        );
        // Position description after placeholder with some spacing
        let desc_x = text_rect.left() + 100.0; // Fixed offset for alignment
        ui.painter().galley(
            egui::pos2(desc_x, text_rect.center().y - desc_galley.size().y / 2.0),
            desc_galley,
            theme.colors.on_surface_variant,
        );

        // Tooltip with full description and example
        let tooltip = if let Some(example) = &var.example {
            format!("{}\n\nExample: {}", var.description, example)
        } else {
            var.description.clone()
        };
        response.clone().on_hover_text(tooltip);

        response.clicked()
    }
}
