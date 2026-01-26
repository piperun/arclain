//! Organization Rules Settings Page
//!
//! CRUD interface for managing organization rules. Part of the Settings feature.

mod add_rule_dialog;
mod rule_editor;

use add_rule_dialog::AddRuleDialog;
pub use rule_editor::{RuleEditorState, RuleEditorAction};
use arclain_core::features::organization::OrganizationRule;
use arclain_core::OrganizationService;
use crate::core::SettingsPage;
use crate::features::settings::domain::types::SettingsAction;
use crate::shared::components::item_table::{ItemTable, TableColumn};
use crate::shared::components::Form;

pub struct RulesPage {
    rules: Option<Vec<OrganizationRule>>,
    dialog: AddRuleDialog,
    error: Option<String>,
    /// State for the rule editor (when editing a rule via dedicated page)
    editor_state: Option<RuleEditorState>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self {
            rules: None,
            dialog: AddRuleDialog::default(),
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
                    if ui.button(format!("{} Add New Rule", egui_phosphor::regular::PLUS)).clicked() {
                        self.dialog.open();
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
                                        .button(format!("{}", egui_phosphor::regular::PENCIL))
                                        .on_hover_text("Edit rule")
                                        .clicked()
                                    {
                                        actions.edit(idx);
                                    }

                                    ui.add_space(4.0);

                                    // Delete button commented out as in original
                                    // if ui
                                    //     .button(format!("{}", egui_phosphor::regular::TRASH))
                                    //     .on_hover_text("Delete rule")
                                    //     .clicked()
                                    // {
                                    //     actions.delete(idx);
                                    // }
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

                // Handle delete action (currently commented out but infrastructure is ready)
                // if let Some(delete_idx) = actions.get_delete() {
                //     if let Some(rules) = &self.rules {
                //         if let Some(rule) = rules.get(*delete_idx) {
                //             if let Err(e) = service.delete_domain_rule(rule.id) {
                //                 self.error = Some(format!("Failed to delete: {}", e));
                //             } else {
                //                 self.rules = None; // Trigger refresh
                //             }
                //         }
                //     }
                // }
            });

        // Handle Dialog (for quick add - still using dialog for new rules)
        if self.dialog.is_open() {
            if let Some(new_rule) = self.dialog.show(ui.ctx(), theme) {
                if let Err(e) = service.save_domain_rule(&new_rule) {
                    self.error = Some(format!("Failed to save rule: {}", e));
                } else {
                    self.rules = None; // Trigger refresh
                }
            }
        }

        action
    }

    /// Render the rule editor page for a specific rule
    pub fn render_edit_rule(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::shared::theme::AppTheme,
        service: &OrganizationService,
        rule_id: i64,
    ) -> Option<RuleEditorAction> {
        // Initialize editor state if not already set or if rule_id changed
        let needs_init = match &self.editor_state {
            None => true,
            Some(state) => state.original_id != rule_id,
        };

        if needs_init {
            if rule_id == 0 {
                // New rule
                self.editor_state = Some(RuleEditorState::new_rule());
            } else {
                // Load existing rule
                match service.get_domain_rule(rule_id) {
                    Ok(Some(rule)) => {
                        self.editor_state = Some(RuleEditorState::new(rule));
                    }
                    Ok(None) => {
                        ui.label("Rule not found");
                        return Some(RuleEditorAction::Cancelled);
                    }
                    Err(e) => {
                        ui.label(format!("Error loading rule: {}", e));
                        return Some(RuleEditorAction::Cancelled);
                    }
                }
            }
        }

        // Render the editor
        if let Some(state) = &mut self.editor_state {
            let action = rule_editor::render_rule_editor(ui, theme, state, service);

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
}
