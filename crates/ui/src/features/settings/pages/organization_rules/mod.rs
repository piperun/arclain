//! Organization Rules Settings Page
//!
//! CRUD interface for managing organization rules. Part of the Settings feature.

mod add_rule_dialog;

use add_rule_dialog::AddRuleDialog;
use arclain_core::config::database::{delete_org_rule, list_org_rules, save_org_rule};
use arclain_core::organization::OrganizationRule;
use arclain_db::SqliteDb;
use egui::Ui;

pub struct RulesPage {
    rules: Option<Vec<OrganizationRule>>,
    dialog: AddRuleDialog,
    error: Option<String>,
}

impl Default for RulesPage {
    fn default() -> Self {
        Self {
            rules: None,
            dialog: AddRuleDialog::default(),
            error: None,
        }
    }
}

impl RulesPage {
    pub fn new() -> Self {
        Self::default()
    }

    fn refresh_rules(&mut self, db: &SqliteDb) {
        match list_org_rules(db) {
            Ok(rules) => self.rules = Some(rules),
            Err(e) => self.error = Some(format!("Failed to load rules: {}", e)),
        }
    }

    pub fn render(&mut self, ui: &mut Ui, db: &SqliteDb) {
        if self.rules.is_none() {
            self.refresh_rules(db);
        }

        ui.heading("Organization Rules");
        ui.label("Manage rules for automatically organizing archives based on metadata.");
        ui.add_space(8.0);

        if ui.button("Add New Rule").clicked() {
            self.dialog.open();
        }

        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::RED, err);
        }

        ui.separator();

        let rule_to_delete = None;
        if let Some(rules) = &self.rules {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for rule in rules {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::LIGHT_BLUE, &rule.name);
                            // if rule.is_system {
                            //     ui.label("(System)");
                            // }
                            if !rule.is_enabled {
                                ui.colored_label(egui::Color32::GRAY, "(Disabled)");
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // if !rule.is_system {
                                    //     if ui.button("Delete").clicked() {
                                    //         rule_to_delete = rule.id;
                                    //     }
                                    // }

                                    if ui.button("Edit").clicked() {
                                        self.dialog.edit(rule.clone());
                                    }
                                },
                            );
                        });

                        // Details
                        ui.horizontal(|ui| {
                            // ui.label(format!("Category: {}", rule.category));
                            if let Some(pattern) = &rule.trigger.filename_pattern {
                                ui.label(format!(" | Pattern: {}", pattern));
                            }
                        });
                        // if let Some(desc) = &rule.description {
                        //     ui.label(egui::RichText::new(desc).italics().weak());
                        // }
                    });
                    ui.add_space(4.0);
                }
            });
        }

        if let Some(id) = rule_to_delete {
            if let Err(e) = delete_org_rule(db, id) {
                self.error = Some(format!("Failed to delete: {}", e));
            } else {
                self.rules = None; // Trigger refresh
            }
        }

        // Handle Dialog
        if self.dialog.is_open() {
            if let Some(new_rule) = self.dialog.show(ui.ctx()) {
                if let Err(e) = save_org_rule(db, &new_rule) {
                    self.error = Some(format!("Failed to save rule: {}", e));
                } else {
                    self.rules = None; // Trigger refresh
                }
            }
        }
    }
}
