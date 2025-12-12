//! Add/Edit Rule Dialog for Organization Rules
//!
//! Dialog UI for creating and editing organization rules.

use arclain_core::organization::OrganizationRule;
use eframe::egui::{self, Align, Layout, Window};

pub struct AddRuleDialog {
    open: bool,
    rule: OrganizationRule,
    is_edit: bool,
    title_error: Option<String>,
}

impl Default for AddRuleDialog {
    fn default() -> Self {
        Self {
            open: false,
            rule: OrganizationRule::default(),
            is_edit: false,
            title_error: None,
        }
    }
}

impl AddRuleDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.is_edit = false;
        self.rule = OrganizationRule::default();
        self.title_error = None;
    }

    pub fn edit(&mut self, rule: OrganizationRule) {
        self.open = true;
        self.is_edit = true;
        self.rule = rule;
        self.title_error = None;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Returns Some(rule) if the user saved
    pub fn show(&mut self, ctx: &egui::Context) -> Option<OrganizationRule> {
        let mut result = None;
        let mut open = self.open;

        Window::new(if self.is_edit {
            "Edit Rule"
        } else {
            "Add Rule"
        })
        .open(&mut open)
        .resize(|r| r.fixed_size((500.0, 600.0)))
        .collapsible(false)
        .show(ctx, |ui| {
            ui.add_space(8.0);

            // Basic Info
            ui.heading("General Information");
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut self.rule.name);
            });
            if let Some(err) = &self.title_error {
                ui.colored_label(egui::Color32::RED, err);
            }

            // ui.horizontal(|ui| {
            //     ui.label("Category:");
            //     ui.text_edit_singleline(&mut self.rule.category);
            // });

            // ui.label("Description:");
            // let mut desc = self.rule.description.clone().unwrap_or_default();
            // ui.text_edit_multiline(&mut desc);
            // self.rule.description = if desc.is_empty() { None } else { Some(desc) };

            ui.add_space(16.0);
            ui.separator();

            // Trigger
            ui.heading("Match Criteria (Trigger)");
            ui.horizontal(|ui| {
                ui.label("Filename Pattern (Regex):");
                let mut pattern = self
                    .rule
                    .trigger
                    .filename_pattern
                    .clone()
                    .unwrap_or_default();
                ui.text_edit_singleline(&mut pattern)
                    .on_hover_text("Regex to match archive filename. Supports match groups.");
                self.rule.trigger.filename_pattern = if pattern.is_empty() {
                    None
                } else {
                    Some(pattern)
                };
            });

            ui.horizontal(|ui| {
                ui.label("Must contain file (Glob):");
                let mut has_file = self.rule.trigger.has_file.clone().unwrap_or_default();
                ui.text_edit_singleline(&mut has_file);
                self.rule.trigger.has_file = if has_file.is_empty() {
                    None
                } else {
                    Some(has_file)
                };
            });

            ui.checkbox(&mut self.rule.is_enabled, "Rule Enabled");

            ui.add_space(16.0);
            ui.separator();

            // Actions
            ui.heading("Organization Actions");
            ui.checkbox(
                &mut self.rule.actions.use_standard_layout,
                "Use Standard Layout",
            )
            .on_hover_text("Automatically flattens game content into 'Game' folder");

            if !self.rule.actions.use_standard_layout {
                ui.label("Custom layout configuration not fully implemented in UI yet.");
                // TODO: Custom moves editor
            }

            if self.rule.actions.use_standard_layout {
                ui.horizontal(|ui| {
                    ui.label("Root Folder Name:");
                    let mut root = self
                        .rule
                        .actions
                        .root_folder
                        .clone()
                        .unwrap_or_else(|| "Game".to_string());
                    ui.text_edit_singleline(&mut root);
                    self.rule.actions.root_folder = Some(root);
                });
            }

            // ui.checkbox(
            //     &mut self.rule.actions.organize_content,
            //     "Organize Content (Extract/Repack)",
            // );
            // ui.checkbox(
            //     &mut self.rule.actions.delete_original,
            //     "Delete Original Archive",
            // );

            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui.button("Save").clicked() {
                    if self.rule.name.trim().is_empty() {
                        self.title_error = Some("Name is required".to_string());
                    } else {
                        result = Some(self.rule.clone());
                        self.open = false;
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.open = false;
                }
            });
        });

        self.open = open;
        result
    }
}
