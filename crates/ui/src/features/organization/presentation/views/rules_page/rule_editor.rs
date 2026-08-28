//! Rule Editor Page
//!
//! Full-page editor for organization rules with template variable support.

use crate::shared::components::{Form, TemplateVariable, VariableGroup, VariablePicker};
use crate::shared::theme::AppTheme;
use arclain_app::organization::{
    FetchSourceDto, GeneratedContentDto, LayoutDto, OrganizationRuleActionsDto,
    OrganizationRuleInput, OrganizationRuleTriggerDto, OutputSelectorDto, PlacementSourceDto,
};
use arclain_widgets::{TextInput, ToggleSwitch};
use eframe::egui;

/// Standard input field width
const FIELD_WIDTH: f32 = 320.0;

/// State for the rule editor
pub struct RuleEditorState {
    /// The rule being edited, in the exact shape a save submits
    /// (`ArclainApp::upsert_organization_rule`) -- the editor mutates
    /// the request it will send rather than a separate model it would
    /// then have to convert.
    pub rule: OrganizationRuleInput,
    /// Original rule ID (0 for new rule), matching the id
    /// `SettingsPage::EditRule` routes on.
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

/// Fields that can receive variable insertions.
///
/// Only the archive name: the layout's own templates (the output folder
/// name, each destination, each fetched file's name) are rendered
/// read-only here, so there is nothing to insert into yet.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RuleField {
    ArchiveName,
}

impl RuleEditorState {
    pub fn new(rule: OrganizationRuleInput) -> Self {
        let original_id = rule
            .id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(0);
        Self {
            rule,
            original_id,
            variable_picker: VariablePicker::new(),
            error: None,
            is_dirty: false,
            target_field: None,
        }
    }

    /// A blank rule: `id: None` so a save creates one, and disabled
    /// until the author turns it on -- the state the pre-facade editor
    /// started from.
    pub fn new_rule() -> Self {
        Self::new(OrganizationRuleInput {
            id: None,
            name: String::new(),
            priority: 0,
            enabled: false,
            trigger: OrganizationRuleTriggerDto::default(),
            actions: OrganizationRuleActionsDto::default(),
        })
    }

    /// Add plugin-provided variables
    pub fn add_plugin_variables(&mut self, plugin_name: &str, variables: Vec<TemplateVariable>) {
        self.variable_picker
            .add_group(VariableGroup::new(plugin_name).with_variables(variables));
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
    Form::new().id("rule_editor").show(ui, theme, |ui| {
        render_rule_form(ui, theme, state);
    });

    // Handle variable picker dialog
    if let Some(var) = state.variable_picker.show(ui.ctx(), theme) {
        if let Some(field) = state.target_field {
            match field {
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

fn render_rule_form(ui: &mut egui::Ui, theme: &AppTheme, state: &mut RuleEditorState) {
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
            .add(ToggleSwitch::new(&mut state.rule.enabled).with_theme_colors(&theme.colors))
            .changed()
        {
            state.is_dirty = true;
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(if state.rule.enabled {
                "Active"
            } else {
                "Inactive"
            })
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
    let mut pattern = state
        .rule
        .trigger
        .filename_pattern
        .clone()
        .unwrap_or_default();
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
        state.rule.trigger.filename_pattern = if pattern.is_empty() {
            None
        } else {
            Some(pattern)
        };
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
        state.rule.trigger.has_file = if has_file.is_empty() {
            None
        } else {
            Some(has_file)
        };
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

    render_layout_detail(ui, theme, &state.rule.actions.layout);
    ui.add_space(12.0);

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
            state.rule.actions.output_name = if output_name.is_empty() {
                None
            } else {
                Some(output_name)
            };
            state.is_dirty = true;
        }

        if ui
            .add(
                arclain_widgets::IconButton::new(egui_phosphor::regular::BRACKETS_CURLY)
                    .size(arclain_widgets::IconButtonSize::Medium)
                    .with_theme_colors(&theme.colors),
            )
            .on_hover_text("Insert variable")
            .clicked()
        {
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
    if !state.rule.actions.layout.name.is_empty() {
        ui.add_space(8.0);
        if ui
            .small_button("Copy folder name to archive name")
            .clicked()
        {
            state.rule.actions.output_name = Some(state.rule.actions.layout.name.clone());
            state.is_dirty = true;
        }
    }
}

/// The saved layout, read-only: what counts as one output, what each is
/// called, where its files go, and what is written or fetched into it.
///
/// Read-only on purpose. A layout is a nested, enum-bearing shape and
/// editing it needs a real editor; a half-built one that silently
/// flattened the parts it could not express would be worse than none,
/// because a rule would then be saved as something other than what its
/// author loaded. What a rule already carries is shown in full so
/// nothing about it is invisible in the meantime.
fn render_layout_detail(ui: &mut egui::Ui, theme: &AppTheme, layout: &LayoutDto) {
    let detail = |ui: &mut egui::Ui, label: &str, value: String| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.label(
                egui::RichText::new(value)
                    .size(12.0)
                    .monospace()
                    .color(theme.colors.on_surface),
            );
        });
    };

    ui.label(
        egui::RichText::new("Layout")
            .size(12.0)
            .strong()
            .color(theme.colors.on_surface),
    );
    ui.add_space(4.0);

    detail(
        ui,
        "One output per: ",
        match &layout.outputs {
            OutputSelectorDto::Whole => "the whole archive".to_string(),
            OutputSelectorDto::PerDirectoryContaining { marker } => {
                format!("folder containing {marker}")
            }
        },
    );
    detail(
        ui,
        "Folder name: ",
        if layout.name.is_empty() {
            "(no wrapper folder)".to_string()
        } else {
            layout.name.clone()
        },
    );

    for variable in &layout.file_variables {
        detail(
            ui,
            "Reads: ",
            format!(
                "${} from {} key {}",
                variable.as_name, variable.file, variable.key
            ),
        );
    }

    if layout.place.is_empty() {
        detail(ui, "Places: ", "nothing".to_string());
    }
    for placement in &layout.place {
        let source = match &placement.from {
            PlacementSourceDto::All => "everything".to_string(),
            PlacementSourceDto::Matching(glob) => format!("files matching {glob}"),
            PlacementSourceDto::ContentRoot => "the detected content folder".to_string(),
        };
        let destination = if placement.into.is_empty() {
            "the output root".to_string()
        } else {
            placement.into.clone()
        };
        detail(ui, "Places: ", format!("{source} into {destination}"));
    }

    for generated in &layout.generate {
        let what = match generated.content {
            GeneratedContentDto::MetadataDocument => "the metadata document",
        };
        detail(ui, "Writes: ", format!("{what} to {}", generated.into));
    }

    for fetched in &layout.fetch {
        let what = match fetched.source {
            FetchSourceDto::Screenshots => "screenshots",
        };
        detail(
            ui,
            "Fetches: ",
            format!("{what} into {} as {}", fetched.into, fetched.name),
        );
    }

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Editing a layout is not available here yet.")
            .size(11.0)
            .color(theme.colors.on_surface_variant),
    );
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
