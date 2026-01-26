//! Rule Editor Page
//!
//! Full-page editor for organization rules with template variable support.

use arclain_core::features::organization::OrganizationRule;
use crate::shared::components::{Form, Switch, VariablePicker, VariableGroup, TemplateVariable};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// State for the rule editor
pub struct RuleEditorState {
    /// The rule being edited (clone for editing)
    pub rule: OrganizationRule,
    /// Original rule ID (0 for new rule)
    pub original_id: i64,
    /// Variable picker dialog state
    pub variable_picker: VariablePicker,
    /// Error message if any
    pub error: Option<String>,
    /// Whether changes have been made
    pub is_dirty: bool,
    /// Which field to insert variable into
    pub target_field: Option<RuleField>,
}

/// Fields that can receive variable insertions
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RuleField {
    FolderName,
    ArchiveName,
}

impl RuleEditorState {
    pub fn new(rule: OrganizationRule) -> Self {
        let original_id = rule.id;
        Self {
            rule,
            original_id,
            variable_picker: VariablePicker::new(),
            error: None,
            is_dirty: false,
            target_field: None,
        }
    }

    pub fn new_rule() -> Self {
        Self::new(OrganizationRule::default())
    }

    /// Add plugin-provided variables
    pub fn add_plugin_variables(&mut self, plugin_name: &str, variables: Vec<TemplateVariable>) {
        self.variable_picker.add_group(
            VariableGroup::new(plugin_name)
                .with_variables(variables)
        );
    }
}

/// Render the rule editor page
/// Note: Save and Cancel buttons are in the page header, not here
pub fn render_rule_editor(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut RuleEditorState,
) -> RuleEditorAction {
    let field_width = 300.0;

    // Error display (if any from previous save attempt)
    if let Some(err) = &state.error {
        ui.colored_label(egui::Color32::RED, err);
        ui.add_space(8.0);
    }

    // Form content
    Form::new()
        .id("rule_editor")
        .show(ui, theme, |ui| {
            render_rule_form(ui, theme, state, field_width);
        });

    // Handle variable picker dialog
    if let Some(var) = state.variable_picker.show(ui.ctx(), theme) {
        if let Some(field) = state.target_field {
            match field {
                RuleField::FolderName => {
                    let current = state.rule.actions.root_folder.clone().unwrap_or_default();
                    state.rule.actions.root_folder = Some(format!("{}{}", current, var));
                    state.is_dirty = true;
                }
                RuleField::ArchiveName => {
                    let current = state.rule.actions.output_name.clone().unwrap_or_default();
                    state.rule.actions.output_name = Some(format!("{}{}", current, var));
                    state.is_dirty = true;
                }
            }
        }
        state.target_field = None;
    }

    RuleEditorAction::None
}

fn render_rule_form(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut RuleEditorState,
    field_width: f32,
) {
    // Basic Info Section
    ui.label(
        egui::RichText::new("Basic Information")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    egui::Grid::new("rule_basic_info")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Name:");
            if ui.add(egui::TextEdit::singleline(&mut state.rule.name).desired_width(field_width)).changed() {
                state.is_dirty = true;
            }
            ui.end_row();

            ui.label("Enabled:");
            if ui.add(Switch::new(&mut state.rule.is_enabled)).changed() {
                state.is_dirty = true;
            }
            ui.end_row();
        });

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(16.0);

    // Conditions Section
    ui.label(
        egui::RichText::new("Conditions")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("When should this rule apply?")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    egui::Grid::new("rule_conditions")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Archive name matches:");
            let mut pattern = state.rule.trigger.filename_pattern.clone().unwrap_or_default();
            let response = ui.add(
                egui::TextEdit::singleline(&mut pattern)
                    .hint_text("regex, e.g. RJ\\d+")
                    .desired_width(field_width),
            );
            if response.changed() {
                state.rule.trigger.filename_pattern = if pattern.is_empty() { None } else { Some(pattern) };
                state.is_dirty = true;
            }
            ui.end_row();

            ui.label("Contains file matching:");
            let mut has_file = state.rule.trigger.has_file.clone().unwrap_or_default();
            let response = ui.add(
                egui::TextEdit::singleline(&mut has_file)
                    .hint_text("glob, e.g. *.exe")
                    .desired_width(field_width),
            );
            if response.changed() {
                state.rule.trigger.has_file = if has_file.is_empty() { None } else { Some(has_file) };
                state.is_dirty = true;
            }
            ui.end_row();
        });

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(16.0);

    // Actions Section
    ui.label(
        egui::RichText::new("Actions")
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("What should happen when this rule matches?")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    // Folder organization
    if ui.checkbox(&mut state.rule.actions.use_standard_layout, "Organize contents into single folder").changed() {
        state.is_dirty = true;
    }

    if state.rule.actions.use_standard_layout {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(24.0); // indent
            ui.label("Folder name:");
            let mut root = state.rule.actions.root_folder.clone().unwrap_or_else(|| "Game".to_string());
            let response = ui.add(
                egui::TextEdit::singleline(&mut root)
                    .hint_text("e.g. {name} or Game")
                    .desired_width(200.0),
            );
            if response.changed() {
                state.rule.actions.root_folder = Some(root);
                state.is_dirty = true;
            }
            // Insert variable button
            if ui.button(egui_phosphor::regular::BRACKETS_CURLY)
                .on_hover_text("Insert variable")
                .clicked()
            {
                state.target_field = Some(RuleField::FolderName);
                state.variable_picker.open();
            }
        });
    }

    ui.add_space(16.0);

    // Output naming
    ui.label(
        egui::RichText::new("Output Naming")
            .size(13.0)
            .color(theme.colors.on_surface),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Archive name:");
        let mut output_name = state.rule.actions.output_name.clone().unwrap_or_default();
        let response = ui.add(
            egui::TextEdit::singleline(&mut output_name)
                .hint_text("leave empty to keep original")
                .desired_width(field_width),
        );
        if response.changed() {
            state.rule.actions.output_name = if output_name.is_empty() { None } else { Some(output_name) };
            state.is_dirty = true;
        }
        // Insert variable button
        if ui.button(egui_phosphor::regular::BRACKETS_CURLY)
            .on_hover_text("Insert variable")
            .clicked()
        {
            state.target_field = Some(RuleField::ArchiveName);
            state.variable_picker.open();
        }
    });

    // Copy button
    ui.add_space(8.0);
    let has_folder = state.rule.actions.root_folder.is_some();

    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.add_enabled_ui(has_folder, |ui| {
            if ui.button("Copy folder name to archive name").clicked() {
                if let Some(folder) = &state.rule.actions.root_folder {
                    state.rule.actions.output_name = Some(folder.clone());
                    state.is_dirty = true;
                }
            }
        });
    });
}

/// Actions returned from the rule editor
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEditorAction {
    None,
    Saved,
    Cancelled,
}
