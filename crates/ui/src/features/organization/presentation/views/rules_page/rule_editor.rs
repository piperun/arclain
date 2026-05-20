//! Rule Editor Page
//!
//! Full-page editor for organization rules with template variable support.

use arclain_core::features::organization::OrganizationRule;
use arclain_widgets::{TextInput, ToggleSwitch};
use crate::shared::components::{Form, VariablePicker, VariableGroup, TemplateVariable};
use crate::shared::theme::AppTheme;
use eframe::egui;

/// Standard input field width
const FIELD_WIDTH: f32 = 320.0;

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
    // Error display (if any from previous save attempt)
    if let Some(err) = &state.error {
        ui.colored_label(egui::Color32::RED, err);
        ui.add_space(8.0);
    }

    // Form content
    Form::new()
        .id("rule_editor")
        .show(ui, theme, |ui| {
            render_rule_form(ui, theme, state);
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
) {
    // Section: Basic Information
    render_section_header(ui, theme, "Basic Information", None);

    // Rule Name
    if TextInput::new(&mut state.rule.name)
        .label("Rule Name")
        .hint("Enter a descriptive name")
        .width(FIELD_WIDTH)
        .with_theme_colors(&theme.colors)
        .show(ui)
        .changed()
    {
        state.is_dirty = true;
    }
    ui.add_space(12.0);

    // Enabled toggle
    ui.label(
        egui::RichText::new("Status")
            .size(12.0)
            .color(theme.colors.on_surface),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add(
                ToggleSwitch::new(&mut state.rule.is_enabled)
                    .with_theme_colors(&theme.colors),
            )
            .changed()
        {
            state.is_dirty = true;
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(if state.rule.is_enabled { "Active" } else { "Inactive" })
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
    });

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(16.0);

    // Section: Conditions
    render_section_header(
        ui,
        theme,
        "Conditions",
        Some("Define when this rule should be triggered"),
    );

    // Filename pattern
    let mut pattern = state.rule.trigger.filename_pattern.clone().unwrap_or_default();
    if TextInput::new(&mut pattern)
        .label("Filename Pattern")
        .hint("Regex pattern, e.g. RJ\\d+")
        .helper_text("Use regular expressions to match archive filenames")
        .width(FIELD_WIDTH)
        .monospace()
        .with_theme_colors(&theme.colors)
        .show(ui)
        .changed()
    {
        state.rule.trigger.filename_pattern = if pattern.is_empty() { None } else { Some(pattern) };
        state.is_dirty = true;
    }
    ui.add_space(12.0);

    // File content matcher
    let mut has_file = state.rule.trigger.has_file.clone().unwrap_or_default();
    if TextInput::new(&mut has_file)
        .label("Contains File")
        .hint("Glob pattern, e.g. *.exe")
        .helper_text("Match archives containing specific files")
        .width(FIELD_WIDTH)
        .monospace()
        .with_theme_colors(&theme.colors)
        .show(ui)
        .changed()
    {
        state.rule.trigger.has_file = if has_file.is_empty() { None } else { Some(has_file) };
        state.is_dirty = true;
    }

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(16.0);

    // Section: Actions
    render_section_header(
        ui,
        theme,
        "Actions",
        Some("Configure what happens when this rule matches"),
    );

    let consolidate_changed = ui
        .horizontal(|ui| {
            let resp = ui.add(
                ToggleSwitch::new(&mut state.rule.actions.use_standard_layout)
                    .with_theme_colors(&theme.colors),
            );
            ui.label("Consolidate into single folder");
            resp.changed()
        })
        .inner;
    if consolidate_changed {
        state.is_dirty = true;
    }
    ui.add_space(12.0);

    // Folder name (only shown when organization is enabled)
    if state.rule.actions.use_standard_layout {
        // Label
        ui.label(
            egui::RichText::new("Folder Name")
                .size(12.0)
                .color(theme.colors.on_surface),
        );
        ui.add_space(4.0);
        // Input + button row
        ui.horizontal(|ui| {
            let mut root = state.rule.actions.root_folder.clone().unwrap_or_else(|| "Game".to_string());
            if TextInput::new(&mut root)
                .hint("e.g. {title} or Game")
                .width(FIELD_WIDTH - 40.0)
                .with_theme_colors(&theme.colors)
                .show(ui)
                .changed()
            {
                state.rule.actions.root_folder = Some(root);
                state.is_dirty = true;
            }

            if ui.add(
                arclain_widgets::IconButton::new(egui_phosphor::regular::BRACKETS_CURLY)
                    .size(arclain_widgets::IconButtonSize::Medium)
                    .with_theme_colors(&theme.colors)
            ).on_hover_text("Insert variable").clicked() {
                state.target_field = Some(RuleField::FolderName);
                state.variable_picker.open();
            }
        });
        ui.add_space(12.0);
    }

    // Archive name - Label
    ui.label(
        egui::RichText::new("Archive Name")
            .size(12.0)
            .color(theme.colors.on_surface),
    );
    ui.add_space(4.0);
    // Input + button row
    ui.horizontal(|ui| {
        let mut output_name = state.rule.actions.output_name.clone().unwrap_or_default();
        if TextInput::new(&mut output_name)
            .hint("Leave empty to keep original")
            .width(FIELD_WIDTH - 40.0)
            .with_theme_colors(&theme.colors)
            .show(ui)
            .changed()
        {
            state.rule.actions.output_name = if output_name.is_empty() { None } else { Some(output_name) };
            state.is_dirty = true;
        }

        if ui.add(
            arclain_widgets::IconButton::new(egui_phosphor::regular::BRACKETS_CURLY)
                .size(arclain_widgets::IconButtonSize::Medium)
                .with_theme_colors(&theme.colors)
        ).on_hover_text("Insert variable").clicked() {
            state.target_field = Some(RuleField::ArchiveName);
            state.variable_picker.open();
        }
    });
    // Helper text below
    ui.label(
        egui::RichText::new("Template for the output archive name")
            .size(11.0)
            .color(theme.colors.on_surface_variant),
    );

    // Copy folder name button
    let has_folder = state.rule.actions.root_folder.is_some() && state.rule.actions.use_standard_layout;
    if has_folder {
        ui.add_space(8.0);
        if ui.small_button("Copy folder name to archive name").clicked() {
            if let Some(folder) = &state.rule.actions.root_folder {
                state.rule.actions.output_name = Some(folder.clone());
                state.is_dirty = true;
            }
        }
    }
}

/// Render a section header with title and optional description
fn render_section_header(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    description: Option<&str>,
) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    if let Some(desc) = description {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(desc)
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
    }
    ui.add_space(12.0);
}

/// Actions returned from the rule editor
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEditorAction {
    None,
    Saved,
    Cancelled,
}
