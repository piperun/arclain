//! Organization Rules Page
//!
//! CRUD interface for managing organization rules. Owned by the
//! Organization feature.
//!
//! Architecture: `render` returns `Option<RulesPageAction>` describing
//! intent — either a data load or a navigation request. `render_edit_rule`
//! returns a `RuleEditorOutput` carrying both the editor's
//! save/cancel/none signal and any data-load intent. The sibling
//! `handle_rules_page_action` function owns the service calls, so render
//! itself never touches the DB or the plugin runtime.

mod rule_editor;

use crate::core::SettingsPage;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;
use arclain_core::features::organization::OrganizationRule;
use arclain_core::OrganizationService;
use arclain_widgets::{ButtonSize, TextButton};
pub use rule_editor::{RuleEditorAction, RuleEditorState};

/// Intents emitted by `RulesPage`'s render functions. Navigation
/// intents are translated by the caller into `SettingsAction::NavigateTo`;
/// data intents flow through `handle_rules_page_action`.
#[derive(Debug, Clone)]
pub enum RulesPageAction {
    /// Refresh the rules list from the OrganizationService. Fired
    /// automatically when `page.rules` is `None`.
    LoadRules,
    /// Load a rule (or initialize a new one if `rule_id == 0`) into the
    /// editor state. Fired when entering the EditRule sub-page.
    LoadRule { rule_id: i64 },
    /// Navigate to a different settings page. Caller translates to
    /// `SettingsAction::NavigateTo`.
    Navigate(SettingsPage),
}

/// Combined output from `render_edit_rule` — the editor's user-facing
/// signal (Save/Cancel/None) plus any data-load intent the page needs
/// dispatched.
#[derive(Default)]
pub struct RuleEditorOutput {
    /// Save/Cancel/None — caller maps Save/Cancel to a navigate-back.
    pub editor_action: Option<RuleEditorAction>,
    /// Page-internal data action (currently always `LoadRule` on first
    /// entry to a fresh rule_id).
    pub data_action: Option<RulesPageAction>,
}

pub struct RulesPage {
    rules: Option<Vec<OrganizationRule>>,
    error: Option<String>,
    /// State for the rule editor (when editing a rule via dedicated page)
    editor_state: Option<RuleEditorState>,
    /// Set by the dispatcher when `LoadRule` fails or returns no rule.
    /// Rendered as a label + emits Cancelled on the next frame.
    editor_load_error: Option<String>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self {
            rules: None,
            error: None,
            editor_state: None,
            editor_load_error: None,
        }
    }
}

impl RulesPage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Currently surfaced page-level error (load-rules failures
    /// etc.). Used by integration tests to assert dispatcher
    /// behavior.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Currently surfaced editor-load error (set by `LoadRule` when
    /// the requested rule is missing or the service errors out).
    pub fn editor_load_error(&self) -> Option<&str> {
        self.editor_load_error.as_deref()
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<RulesPageAction> {
        // First render (or after a mutation that invalidated the cache):
        // emit a Load action and show a placeholder. The dispatcher
        // populates `self.rules` synchronously after render returns;
        // the next frame shows real data.
        if self.rules.is_none() {
            ui.label(
                egui::RichText::new("Loading rules…")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            return Some(RulesPageAction::LoadRules);
        }

        let mut emitted: Option<RulesPageAction> = None;

        Form::new().id("organization_rules").show(ui, theme, |ui| {
            // Page header
            ui.label(
                egui::RichText::new("Organization Rules")
                    .size(18.0)
                    .strong()
                    .color(theme.colors.on_surface),
            );
            ui.label(
                egui::RichText::new(
                    "Manage rules for automatically organizing archives based on metadata.",
                )
                .size(12.0)
                .color(theme.colors.on_surface_variant),
            );
            ui.add_space(12.0);

            // Header with count and Add button
            ui.horizontal(|ui| {
                if ui
                    .add(
                        TextButton::new(
                            format!("{} Add New Rule", egui_phosphor::regular::PLUS),
                            ButtonSize::Medium,
                        )
                        .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    emitted = Some(RulesPageAction::Navigate(SettingsPage::EditRule(0)));
                }

                if let Some(rules) = &self.rules {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} rules", rules.len()))
                                .size(12.0)
                                .color(theme.colors.on_surface_variant),
                        );
                    });
                }
            });

            // Error display
            if let Some(err) = &self.error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::RED, err);
            }

            ui.add_space(12.0);

            // Table
            let actions = if let Some(rules) = &self.rules {
                let columns = vec![
                    TableColumn::exact(60.0, "Status"),
                    TableColumn::resizable(180.0, "Name"),
                    TableColumn::remainder("Pattern"),
                    TableColumn::exact(90.0, "Actions").align_right(),
                ];

                ItemTable::new()
                    .empty_message("No organization rules configured yet.")
                    .show(ui, theme, &columns, rules, |rule, idx, row, actions| {
                        // Status column
                        row.col(|ui| {
                            if rule.is_enabled {
                                ui.label(
                                    egui::RichText::new(egui_phosphor::regular::CHECK_CIRCLE)
                                        .color(theme.colors.primary),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(egui_phosphor::regular::X_CIRCLE)
                                        .color(theme.colors.on_surface_variant),
                                );
                            }
                        });

                        // Name column
                        row.col(|ui| {
                            ui.label(egui::RichText::new(&rule.name).color(if rule.is_enabled {
                                theme.colors.on_surface
                            } else {
                                theme.colors.on_surface_variant
                            }));
                        });

                        // Pattern column
                        row.col(|ui| {
                            if let Some(pattern) = &rule.trigger.filename_pattern {
                                ui.label(
                                    egui::RichText::new(pattern)
                                        .family(egui::FontFamily::Monospace)
                                        .color(theme.colors.on_surface_variant),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("—").color(theme.colors.on_surface_variant),
                                );
                            }
                        });

                        // Actions column
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(
                                            TextButton::new(
                                                egui_phosphor::regular::PENCIL,
                                                ButtonSize::Small,
                                            )
                                            .with_theme_colors(&theme.colors),
                                        )
                                        .on_hover_text("Edit rule")
                                        .clicked()
                                    {
                                        actions.edit(idx);
                                    }

                                    ui.add_space(4.0);
                                },
                            );
                        });
                    })
            } else {
                let empty_rules: Vec<OrganizationRule> = Vec::new();
                ItemTable::new().show(ui, theme, &[], &empty_rules, |_, _, _, _| {})
            };

            // Handle deferred edit click → emit Navigate action.
            if emitted.is_none() {
                if let Some(edit_idx) = actions.get_edit() {
                    if let Some(rules) = &self.rules {
                        if let Some(rule) = rules.get(*edit_idx) {
                            emitted =
                                Some(RulesPageAction::Navigate(SettingsPage::EditRule(rule.id)));
                        }
                    }
                }
            }
        });

        emitted
    }

    /// Render the rule editor page for a specific rule. Returns a
    /// combined output: the editor's Save/Cancel/None signal plus any
    /// page-internal data load that needs dispatching.
    pub fn render_edit_rule(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        rule_id: i64,
    ) -> RuleEditorOutput {
        let mut output = RuleEditorOutput::default();

        // If the dispatcher couldn't load the rule on a previous frame,
        // surface the error and bail out via Cancelled.
        if let Some(err) = self.editor_load_error.take() {
            ui.label(err);
            output.editor_action = Some(RuleEditorAction::Cancelled);
            return output;
        }

        // Need to (re)initialize editor state? Emit LoadRule and show a
        // placeholder. The dispatcher populates `self.editor_state` after
        // render returns; the next frame renders the actual editor.
        let needs_init = match &self.editor_state {
            None => true,
            Some(state) => state.original_id != rule_id,
        };
        if needs_init {
            ui.label(
                egui::RichText::new("Loading rule…")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            output.data_action = Some(RulesPageAction::LoadRule { rule_id });
            return output;
        }

        // Render the editor (Save/Cancel are handled by the parent header).
        if let Some(state) = &mut self.editor_state {
            let action = rule_editor::render_rule_editor(ui, theme, state);

            match action {
                RuleEditorAction::Saved | RuleEditorAction::Cancelled => {
                    // Editor done: clear local state, invalidate the
                    // list cache so the next list render re-fetches.
                    self.editor_state = None;
                    self.rules = None;
                    output.editor_action = Some(action);
                }
                RuleEditorAction::None => {}
            }
        }

        output
    }

    /// Clear the editor state (call when navigating away)
    pub fn clear_editor(&mut self) {
        self.editor_state = None;
    }

    /// Check if the editor has unsaved changes
    pub fn is_editor_dirty(&self) -> bool {
        self.editor_state
            .as_ref()
            .map(|s| s.is_dirty)
            .unwrap_or(false)
    }

    /// Save the current rule being edited. Called from outside render
    /// (the settings header save button), so this remains a direct
    /// service call rather than going through the action enum.
    pub fn save_editor_rule(&mut self, service: &OrganizationService) -> Result<(), String> {
        let state = self
            .editor_state
            .as_mut()
            .ok_or_else(|| "No rule being edited".to_string())?;

        service
            .save_domain_rule(&state.rule)
            .map_err(|e| format!("Failed to save: {}", e))?;

        state.is_dirty = false;
        Ok(())
    }

    /// Mark the rule as saved and clear editor state (called after successful save)
    pub fn mark_saved_and_clear(&mut self) {
        self.editor_state = None;
        self.rules = None; // Trigger refresh when returning to list
    }
}

/// Dispatch a `RulesPageAction` against the OrganizationService and
/// update the page's cached state. Called by the parent view after
/// `render` or `render_edit_rule` returns a data action.
///
/// All side effects on the service live here, so the render functions
/// stay pure intent-emitters.
pub fn handle_rules_page_action(
    page: &mut RulesPage,
    action: RulesPageAction,
    service: &OrganizationService,
    plugins: Option<&[crate::features::plugins::domain::types::PluginInfo]>,
) {
    match action {
        RulesPageAction::Navigate(_) => {
            // Navigation is the caller's responsibility — it translates to
            // `SettingsAction::NavigateTo` and returns up the chain. The
            // dispatcher should never be called with this variant.
            debug_assert!(
                false,
                "RulesPageAction::Navigate should be handled by the caller, not the data dispatcher"
            );
        }
        RulesPageAction::LoadRules => match service.list_domain_rules() {
            Ok(rules) => {
                page.rules = Some(rules);
                page.error = None;
            }
            Err(e) => {
                page.error = Some(format!("Failed to load rules: {}", e));
            }
        },
        RulesPageAction::LoadRule { rule_id } => {
            let mut editor_state = if rule_id == 0 {
                RuleEditorState::new_rule()
            } else {
                match service.get_domain_rule(rule_id) {
                    Ok(Some(rule)) => RuleEditorState::new(rule),
                    Ok(None) => {
                        page.editor_load_error = Some("Rule not found".to_string());
                        return;
                    }
                    Err(e) => {
                        page.editor_load_error = Some(format!("Error loading rule: {}", e));
                        return;
                    }
                }
            };

            // Load plugin-provided variables.
            if let Some(plugins) = plugins {
                load_plugin_variables(&mut editor_state, plugins);
            }

            page.editor_state = Some(editor_state);
            page.editor_load_error = None;
        }
    }
}

/// Load template variables from plugins into a fresh editor state.
/// Called from the dispatcher when initializing the editor.
fn load_plugin_variables(
    editor_state: &mut RuleEditorState,
    plugins: &[crate::features::plugins::domain::types::PluginInfo],
) {
    use crate::shared::components::{TemplateVariable, VariableGroup};

    // Query each loaded plugin for template variables.
    for plugin in plugins {
        // Check if plugin provides template variables. This would be via
        // a trait or capability query; for now, add DLSite variables if
        // that plugin is loaded.
        if plugin.name.to_lowercase().contains("dlsite")
            || plugin.name.to_lowercase().contains("rj")
        {
            editor_state.variable_picker.add_group(
                VariableGroup::new("DLSite")
                    .with_id("dlsite")
                    .with_variables(vec![
                        TemplateVariable::new("product_id", "DLSite product code")
                            .with_example("RJ123456"),
                        TemplateVariable::new("title", "Product title from DLSite")
                            .with_example("Game Title"),
                        TemplateVariable::new("circle", "Creator/circle name")
                            .with_example("Circle Name"),
                        TemplateVariable::new("release_date", "Release date")
                            .with_example("2024-01-15"),
                        TemplateVariable::new("tags", "Product tags").with_example("RPG, Fantasy"),
                    ]),
            );
        }
    }
}
