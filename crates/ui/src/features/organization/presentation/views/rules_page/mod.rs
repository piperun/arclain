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
use crate::features::organization::application::facade;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;
use crate::shared::{LoadSlot, SharedState};
use arclain_app::organization::{OrganizationRuleInput, OrganizationRuleSummary};
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
    /// The cached rule list plus the arming state of the auto-fired
    /// `LoadRules` intent — see [`LoadSlot`] for why a failed load
    /// holds (with a Retry affordance) instead of re-firing per frame.
    rules: LoadSlot<Vec<OrganizationRuleSummary>>,
    /// The last `LoadRules` failure, shown in place of the list while
    /// the slot is empty (beside a Retry button that re-arms it).
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
            rules: LoadSlot::default(),
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
    /// the requested rule is missing or the application errors out).
    pub fn editor_load_error(&self) -> Option<&str> {
        self.editor_load_error.as_deref()
    }

    /// The cached rule list. `None` until the dispatcher has run
    /// `LoadRules` successfully at least once, matching
    /// `ProfilesPage::profiles`.
    pub fn rules(&self) -> Option<&[OrganizationRuleSummary]> {
        self.rules.data().map(Vec::as_slice)
    }

    /// The rule the editor is currently editing, as the save will
    /// submit it. `None` when no rule is open in the editor.
    pub fn editor_rule_mut(&mut self) -> Option<&mut OrganizationRuleInput> {
        self.editor_state.as_mut().map(|state| &mut state.rule)
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
    ) -> Option<RulesPageAction> {
        // First render (or after a mutation that invalidated the cache):
        // emit a Load action and show a placeholder. The dispatcher
        // populates `self.rules` synchronously after render returns;
        // the next frame shows real data. The slot arms the intent at
        // most once per user action, so a load the dispatcher *fails*
        // holds on the error branch below instead of re-firing a
        // blocking database call every frame.
        if self.rules.data().is_none() {
            if self.rules.try_fire() {
                ui.label(
                    egui::RichText::new("Loading rules…")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
                return Some(RulesPageAction::LoadRules);
            }
            if let Some(error) = self.error.clone() {
                ui.colored_label(egui::Color32::RED, error);
                ui.add_space(8.0);
                if ui
                    .add(
                        TextButton::new("Retry", ButtonSize::Small)
                            .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    // One fresh shot; the auto-fire above emits it on
                    // the next frame.
                    self.error = None;
                    self.rules.rearm();
                }
            } else {
                // Fired but unanswered — the dispatcher runs in the
                // same frame, so this only shows when no dispatcher is
                // wired at all.
                ui.label(
                    egui::RichText::new("Loading rules…")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
            }
            return None;
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

                if let Some(rules) = self.rules.data() {
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
            let actions = if let Some(rules) = self.rules.data() {
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
                            if rule.enabled {
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
                            ui.label(egui::RichText::new(&rule.name).color(if rule.enabled {
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
                let empty_rules: Vec<OrganizationRuleSummary> = Vec::new();
                ItemTable::new().show(ui, theme, &[], &empty_rules, |_, _, _, _| {})
            };

            // Handle deferred edit click → emit Navigate action. The
            // route carries the numeric row id `SettingsPage::EditRule`
            // is built on; a summary whose id is not one (which the
            // facade never produces) simply has no edit route.
            if emitted.is_none() {
                if let Some(edit_idx) = actions.get_edit() {
                    if let Some(rules) = self.rules.data() {
                        if let Some(id) = rules
                            .get(*edit_idx)
                            .and_then(|rule| rule.id.parse::<i64>().ok())
                        {
                            emitted = Some(RulesPageAction::Navigate(SettingsPage::EditRule(id)));
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
                    self.rules.invalidate();
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
    /// call rather than going through the action enum.
    pub fn save_editor_rule(&mut self, shared: &SharedState) -> Result<(), String> {
        let state = self
            .editor_state
            .as_mut()
            .ok_or_else(|| "No rule being edited".to_string())?;
        let (app, runtime) = facade::handles(shared).ok_or_else(facade::unavailable)?;

        runtime
            .block_on(app.upsert_organization_rule(state.rule.clone()))
            .map_err(|error| facade::describe("Failed to save", &error))?;

        state.is_dirty = false;
        Ok(())
    }

    /// Mark the rule as saved and clear editor state (called after successful save)
    pub fn mark_saved_and_clear(&mut self) {
        self.editor_state = None;
        // Trigger (and arm) one refresh when returning to the list.
        self.rules.invalidate();
    }
}

/// Dispatch a `RulesPageAction` against the application facade and
/// update the page's cached state. Called by the parent view after
/// `render` or `render_edit_rule` returns a data action.
///
/// All side effects live here, so the render functions stay pure
/// intent-emitters.
pub fn handle_rules_page_action(
    page: &mut RulesPage,
    action: RulesPageAction,
    shared: &SharedState,
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
        RulesPageAction::LoadRules => {
            // On failure the cache stays empty and the page's slot
            // stays disarmed: render shows `page.error` with a Retry
            // button instead of re-emitting this action every frame.
            let Some((app, runtime)) = facade::handles(shared) else {
                page.error = Some(facade::unavailable());
                return;
            };
            match runtime.block_on(app.organization_rules()) {
                Ok(rules) => {
                    page.rules.set(rules);
                    page.error = None;
                }
                Err(error) => {
                    page.error = Some(facade::describe("Failed to load rules", &error));
                }
            }
        }
        RulesPageAction::LoadRule { rule_id } => {
            // A brand new rule needs nothing loaded, so it does not need
            // the application at all.
            let mut editor_state = if rule_id == 0 {
                RuleEditorState::new_rule()
            } else {
                let Some((app, runtime)) = facade::handles(shared) else {
                    page.editor_load_error = Some(facade::unavailable());
                    return;
                };
                // The facade exposes no single-rule read: there are a
                // handful of rules and the list is the same query, so
                // this picks the one it wants out of it rather than
                // asking for a lookup endpoint that would exist for one
                // caller.
                match runtime.block_on(app.organization_rules()) {
                    Ok(rules) => match rules
                        .iter()
                        .find(|rule| rule.id.parse::<i64>().ok() == Some(rule_id))
                    {
                        Some(rule) => RuleEditorState::new(to_input(rule)),
                        None => {
                            page.editor_load_error = Some("Rule not found".to_string());
                            return;
                        }
                    },
                    Err(error) => {
                        page.editor_load_error =
                            Some(facade::describe("Error loading rule", &error));
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

/// A saved rule as the editor's in-progress edit of it: every field
/// round-trips, so saving an untouched rule stores exactly what was
/// loaded (the summary and the input mirror each other field for
/// field).
fn to_input(rule: &OrganizationRuleSummary) -> OrganizationRuleInput {
    OrganizationRuleInput {
        id: Some(rule.id.clone()),
        name: rule.name.clone(),
        priority: rule.priority,
        enabled: rule.enabled,
        trigger: rule.trigger.clone(),
        actions: rule.actions.clone(),
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
