//! Organization Rules Settings Page
//!
//! CRUD interface for managing organization rules. Part of the Settings feature.

mod rule_editor;

pub use rule_editor::{RuleEditorState, RuleEditorAction};
use arclain_core::features::organization::OrganizationRule;
use arclain_core::OrganizationService;
use crate::core::SettingsPage;
use crate::features::settings::domain::types::SettingsAction;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;
use arclain_widgets::{ButtonSize, TextButton};

pub struct RulesPage {
    rules: Option<Vec<OrganizationRule>>,
    error: Option<String>,
    /// State for the rule editor (when editing a rule via dedicated page)
    editor_state: Option<RuleEditorState>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self {
            rules: None,
            error: None,
            editor_state: None,
        }
    }
}

impl RulesPage {
    pub fn new() -> Self {
        Self::default()
    }

    fn refresh_rules(&mut self, service: &OrganizationService) {
        match service.list_domain_rules() {
            Ok(rules) => self.rules = Some(rules),
            Err(e) => self.error = Some(format!("Failed to load rules: {}", e)),
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        service: &OrganizationService,
    ) -> Option<SettingsAction> {
        let mut action: Option<SettingsAction> = None;
        if self.rules.is_none() {
            self.refresh_rules(service);
        }

        Form::new()
            .id("organization_rules")
            .show(ui, theme, |ui| {
                // Page header
                ui.label(
                    egui::RichText::new("Organization Rules")
                        .size(18.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );
                ui.label(
                    egui::RichText::new("Manage rules for automatically organizing archives based on metadata.")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(12.0);

                // Header with count and Add button
                ui.horizontal(|ui| {
                    if ui.add(TextButton::new(format!("{} Add New Rule", egui_phosphor::regular::PLUS), ButtonSize::Medium).with_theme_colors(&theme.colors)).clicked() {
                        action = Some(SettingsAction::NavigateTo(SettingsPage::EditRule(0)));
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
                                ui.label(
                                    egui::RichText::new(&rule.name).color(
                                        if rule.is_enabled {
                                            theme.colors.on_surface
                                        } else {
                                            theme.colors.on_surface_variant
                                        },
                                    ),
                                );
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
                                        egui::RichText::new("—")
                                            .color(theme.colors.on_surface_variant),
                                    );
                                }
                            });

                            // Actions column
                            row.col(|ui| {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui
                                        .add(TextButton::new(format!("{}", egui_phosphor::regular::PENCIL), ButtonSize::Small).with_theme_colors(&theme.colors))
                                        .on_hover_text("Edit rule")
                                        .clicked()
                                    {
                                        actions.edit(idx);
                                    }

                                    ui.add_space(4.0);
                                });
                            });
                        })
                } else {
                    let empty_rules: Vec<OrganizationRule> = Vec::new();
                    ItemTable::new().show(ui, theme, &[], &empty_rules, |_, _, _, _| {})
                };

                // Handle deferred actions - navigate to rule editor page
                if let Some(edit_idx) = actions.get_edit() {
                    if let Some(rules) = &self.rules {
                        if let Some(rule) = rules.get(*edit_idx) {
                            action = Some(SettingsAction::NavigateTo(SettingsPage::EditRule(rule.id)));
                        }
                    }
                }

            });

        action
    }

    /// Render the rule editor page for a specific rule
    pub fn render_edit_rule(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        service: &OrganizationService,
        rule_id: i64,
        plugin_manager: Option<&arclain_plugins::PluginManager>,
    ) -> Option<RuleEditorAction> {
        // Initialize editor state if not already set or if rule_id changed
        let needs_init = match &self.editor_state {
            None => true,
            Some(state) => state.original_id != rule_id,
        };

        if needs_init {
            let mut editor_state = if rule_id == 0 {
                // New rule
                RuleEditorState::new_rule()
            } else {
                // Load existing rule
                match service.get_domain_rule(rule_id) {
                    Ok(Some(rule)) => RuleEditorState::new(rule),
                    Ok(None) => {
                        ui.label("Rule not found");
                        return Some(RuleEditorAction::Cancelled);
                    }
                    Err(e) => {
                        ui.label(format!("Error loading rule: {}", e));
                        return Some(RuleEditorAction::Cancelled);
                    }
                }
            };

            // Load plugin-provided variables
            if let Some(pm) = plugin_manager {
                Self::load_plugin_variables(&mut editor_state, pm);
            }

            self.editor_state = Some(editor_state);
        }

        // Render the editor (Save/Cancel are handled in the header)
        if let Some(state) = &mut self.editor_state {
            let action = rule_editor::render_rule_editor(ui, theme, state);

            match action {
                RuleEditorAction::Saved | RuleEditorAction::Cancelled => {
                    self.editor_state = None;
                    self.rules = None; // Trigger refresh when returning to list
                    return Some(action);
                }
                RuleEditorAction::None => {}
            }
        }

        None
    }

    /// Clear the editor state (call when navigating away)
    pub fn clear_editor(&mut self) {
        self.editor_state = None;
    }

    /// Check if the editor has unsaved changes
    pub fn is_editor_dirty(&self) -> bool {
        self.editor_state.as_ref().map(|s| s.is_dirty).unwrap_or(false)
    }

    /// Save the current rule being edited
    pub fn save_editor_rule(&mut self, service: &OrganizationService) -> Result<(), String> {
        let state = self.editor_state.as_mut()
            .ok_or_else(|| "No rule being edited".to_string())?;

        service.save_domain_rule(&state.rule)
            .map_err(|e| format!("Failed to save: {}", e))?;

        state.is_dirty = false;
        Ok(())
    }

    /// Mark the rule as saved and clear editor state (called after successful save)
    pub fn mark_saved_and_clear(&mut self) {
        self.editor_state = None;
        self.rules = None; // Trigger refresh when returning to list
    }

    /// Load template variables from plugins
    fn load_plugin_variables(
        editor_state: &mut RuleEditorState,
        plugin_manager: &arclain_plugins::PluginManager,
    ) {
        use crate::shared::components::{TemplateVariable, VariableGroup};

        // Query each loaded plugin for template variables
        for plugin in plugin_manager.list_plugins() {
            // Check if plugin provides template variables
            // This would be via a trait or capability query
            // For now, we'll add DLSite variables if that plugin is loaded
            if plugin.manifest.plugin.name.to_lowercase().contains("dlsite")
                || plugin.manifest.plugin.name.to_lowercase().contains("rj")
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
                            TemplateVariable::new("tags", "Product tags")
                                .with_example("RPG, Fantasy"),
                        ]),
                );
            }
        }
    }
}
