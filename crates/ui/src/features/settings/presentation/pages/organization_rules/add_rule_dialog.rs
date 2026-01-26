//! Add/Edit Rule Dialog for Organization Rules
//!
//! Dialog UI for creating and editing organization rules.

use arclain_core::features::organization::OrganizationRule;
use crate::shared::components::Switch;
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
            .size(420.0, 380.0);

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
        let field_width = 240.0;

        // Rule name and enabled in a grid
        egui::Grid::new("rule_basic")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name:");
                ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(field_width));
                ui.end_row();

                if let Some(err) = title_error {
                    ui.label("");
                    ui.colored_label(egui::Color32::RED, err);
                    ui.end_row();
                }

                ui.label("Enabled:");
                ui.add(Switch::new(&mut rule.is_enabled));
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // When to apply this rule
        ui.label(
            egui::RichText::new("Conditions")
                .size(13.0)
                .strong()
                .color(theme.colors.on_surface),
        );
        ui.add_space(8.0);

        egui::Grid::new("rule_triggers")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Archive name:");
                let mut pattern = rule.trigger.filename_pattern.clone().unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut pattern)
                        .hint_text("regex pattern, e.g. RJ\\d+")
                        .desired_width(field_width),
                );
                rule.trigger.filename_pattern = if pattern.is_empty() { None } else { Some(pattern) };
                ui.end_row();

                ui.label("Contains file:");
                let mut has_file = rule.trigger.has_file.clone().unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut has_file)
                        .hint_text("glob pattern, e.g. *.exe")
                        .desired_width(field_width),
                );
                rule.trigger.has_file = if has_file.is_empty() { None } else { Some(has_file) };
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // Actions
        ui.label(
            egui::RichText::new("Actions")
                .size(13.0)
                .strong()
                .color(theme.colors.on_surface),
        );
        ui.add_space(8.0);

        ui.checkbox(&mut rule.actions.use_standard_layout, "Organize into single folder");

        if rule.actions.use_standard_layout {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0); // indent
                ui.label("Folder name:");
                let mut root = rule.actions.root_folder.clone().unwrap_or_else(|| "Game".to_string());
                ui.add(egui::TextEdit::singleline(&mut root).desired_width(120.0));
                rule.actions.root_folder = Some(root);
            });
        }
    }
}
