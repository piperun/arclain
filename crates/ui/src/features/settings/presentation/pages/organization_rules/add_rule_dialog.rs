//! Add/Edit Rule Dialog for Organization Rules
//!
//! Dialog UI for creating and editing organization rules.

use arclain_core::features::organization::OrganizationRule;
use crate::shared::components::{FormField, Padding, Section};
use crate::shared::dialogs::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
use eframe::egui;

pub struct AddRuleDialog {
    dialog: FormDialog,
    rule: OrganizationRule,
    title_error: Option<String>,
}

impl Default for AddRuleDialog {
    fn default() -> Self {
        let config = FormDialogConfig::new("Add Rule", "Edit Rule")
            .mode(DialogMode::FixedCenter)
            .size(420.0, 360.0);

        Self {
            dialog: FormDialog::new(config),
            rule: OrganizationRule::default(),
            title_error: None,
        }
    }
}

impl AddRuleDialog {
    pub fn open(&mut self) {
        self.rule = OrganizationRule::default();
        self.title_error = None;
        self.dialog.open_add();
    }

    pub fn edit(&mut self, rule: OrganizationRule) {
        self.rule = rule;
        self.title_error = None;
        self.dialog.open_edit();
    }

    pub fn is_open(&self) -> bool {
        self.dialog.is_open()
    }

    /// Returns Some(rule) if the user saved
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<OrganizationRule> {
        let can_save = !self.rule.name.trim().is_empty();

        // Borrow rule and title_error separately to avoid borrowing self in the closure
        let rule = &mut self.rule;
        let title_error = &self.title_error;

        match self.dialog.show(ctx, theme, can_save, |ui| {
            Self::render_form_content(ui, theme, rule, title_error);
            Some(rule.clone())
        }) {
            FormDialogResult::Save(rule) => Some(rule),
            FormDialogResult::Cancel => None,
            FormDialogResult::None => None,
        }
    }

    fn render_form_content(
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        rule: &mut OrganizationRule,
        title_error: &Option<String>,
    ) {
        // General Section
        Section::new("General").show(ui, theme, |ui| {
            FormField::new("Name").show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(240.0));
            });

            if let Some(err) = title_error {
                Padding::left(124.0).show(ui, |ui| {
                    ui.colored_label(egui::Color32::RED, err);
                });
            }

            ui.checkbox(&mut rule.is_enabled, "Enabled");
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Match Criteria Section
        Section::new("Match Criteria").show(ui, theme, |ui| {
            FormField::new("Filename Pattern").show(ui, |ui| {
                let mut pattern = rule.trigger.filename_pattern.clone().unwrap_or_default();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut pattern)
                        .hint_text("Regex")
                        .desired_width(240.0),
                );
                resp.on_hover_text("Regex to match archive filename");
                rule.trigger.filename_pattern = if pattern.is_empty() { None } else { Some(pattern) };
            });

            FormField::new("Must Contain File").show(ui, |ui| {
                let mut has_file = rule.trigger.has_file.clone().unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut has_file)
                        .hint_text("Glob")
                        .desired_width(240.0),
                );
                rule.trigger.has_file = if has_file.is_empty() { None } else { Some(has_file) };
            });
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Actions Section
        Section::new("Actions").show(ui, theme, |ui| {
            ui.checkbox(&mut rule.actions.use_standard_layout, "Use Standard Layout")
                .on_hover_text("Automatically flattens game content into a root folder");

            if rule.actions.use_standard_layout {
                ui.add_space(4.0);
                FormField::new("Root Folder").show(ui, |ui| {
                    let mut root = rule.actions.root_folder.clone().unwrap_or_else(|| "Game".to_string());
                    ui.add(egui::TextEdit::singleline(&mut root).desired_width(240.0));
                    rule.actions.root_folder = Some(root);
                });
            }
        });
    }
}
