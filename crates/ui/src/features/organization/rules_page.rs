use crate::core::AppState;
use crate::shared::theme::AppTheme;
use arclain_core::config::database as config_db;
use arclain_core::organization::{MoveFileRule, OrganizationRule, RuleActions, RuleTrigger};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Default)]
pub struct OrganizationRulesState {
    pub rules: Vec<OrganizationRule>,
    pub editing_rule: Option<OrganizationRule>,
    pub show_editor: bool,
}


pub fn render(
    ctx: &egui::Context,
    _theme: &AppTheme,
    state: &mut OrganizationRulesState,
    app_state: &Arc<Mutex<AppState>>,
) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Organization Rules");
        ui.add_space(10.0);

        // Toolbar
        ui.horizontal(|ui| {
            if ui.button("➕ Add Rule").clicked() {
                state.editing_rule = Some(OrganizationRule {
                    id: None,
                    name: "New Rule".to_string(),
                    description: None,
                    category: "General".to_string(),
                    priority: 0,
                    is_enabled: true,
                    is_system: false,
                    trigger: RuleTrigger::default(),
                    actions: RuleActions::default(),
                });
                state.show_editor = true;
            }
            if ui.button("📥 Import").clicked() {
                // TODO: Implement import
            }
            if ui.button("📤 Export").clicked() {
                // TODO: Implement export
            }
        });

        ui.add_space(10.0);

        // Rule List
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Load rules if empty (and not just initialized)
            // Ideally we should load this once when entering the page, but for now lazy load
            if state.rules.is_empty() {
                let st = app_state.lock();
                if let Some(p) = &st.db_paths {
                    if let Ok(cfg_db) = config_db::ConfigDb::open(&p.config_db) {
                        if let Ok(rules) = config_db::list_org_rules(&cfg_db.into_sqlite_db()) {
                            state.rules = rules;
                        }
                    }
                }
            }

            let mut delete_id = None;

            for rule in &mut state.rules {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.heading(&rule.name);
                            if let Some(desc) = &rule.description {
                                ui.label(egui::RichText::new(desc).italics());
                            }
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Category: {}", rule.category))
                                        .small(),
                                );
                                ui.label(
                                    egui::RichText::new(format!("Priority: {}", rule.priority))
                                        .small(),
                                );
                            });
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("🗑").clicked()
                                && !rule.is_system {
                                    delete_id = rule.id;
                                }
                            if ui.button("✏").clicked() {
                                state.editing_rule = Some(rule.clone());
                                state.show_editor = true;
                            }
                            ui.checkbox(&mut rule.is_enabled, "Enabled");
                        });
                    });
                });
                ui.add_space(5.0);
            }

            // Handle deletion
            if let Some(id) = delete_id {
                let st = app_state.lock();
                if let Some(p) = &st.db_paths {
                    if let Ok(cfg_db) = config_db::ConfigDb::open(&p.config_db) {
                        let _ = config_db::delete_org_rule(&cfg_db.into_sqlite_db(), id);
                    }
                }
                // Refresh list
                if let Some(p) = &st.db_paths {
                    if let Ok(cfg_db) = config_db::ConfigDb::open(&p.config_db) {
                        if let Ok(rules) = config_db::list_org_rules(&cfg_db.into_sqlite_db()) {
                            state.rules = rules;
                        }
                    }
                }
            }
        });

        // Editor Dialog
        if state.show_editor {
            if let Some(rule) = &mut state.editing_rule {
                egui::Window::new("Edit Rule")
                    .collapsible(false)
                    .resizable(true)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.heading("General");
                            ui.horizontal(|ui| {
                                ui.label("Name:");
                                ui.text_edit_singleline(&mut rule.name);
                            });
                            ui.horizontal(|ui| {
                                ui.label("Description:");
                                let mut desc = rule.description.clone().unwrap_or_default();
                                ui.text_edit_singleline(&mut desc);
                                rule.description = if desc.is_empty() { None } else { Some(desc) };
                            });
                            ui.horizontal(|ui| {
                                ui.label("Category:");
                                ui.text_edit_singleline(&mut rule.category);
                            });
                            ui.horizontal(|ui| {
                                ui.label("Priority:");
                                ui.add(egui::DragValue::new(&mut rule.priority));
                            });

                            ui.separator();
                            ui.heading("Trigger");
                            ui.horizontal(|ui| {
                                ui.label("Filename Pattern (Regex):");
                                let mut pattern =
                                    rule.trigger.filename_pattern.clone().unwrap_or_default();
                                ui.text_edit_singleline(&mut pattern);
                                rule.trigger.filename_pattern = if pattern.is_empty() {
                                    None
                                } else {
                                    Some(pattern)
                                };
                            });
                            ui.horizontal(|ui| {
                                ui.label("Has File (Glob):");
                                let mut file = rule.trigger.has_file.clone().unwrap_or_default();
                                ui.text_edit_singleline(&mut file);
                                rule.trigger.has_file =
                                    if file.is_empty() { None } else { Some(file) };
                            });

                            ui.separator();
                            ui.heading("Actions");
                            ui.horizontal(|ui| {
                                ui.label("Root Folder Template:");
                                let mut root = rule.actions.root_folder.clone().unwrap_or_default();
                                ui.text_edit_singleline(&mut root);
                                rule.actions.root_folder =
                                    if root.is_empty() { None } else { Some(root) };
                            });

                            ui.label("Move Rules:");
                            let mut remove_idx = None;
                            for (i, move_rule) in rule.actions.move_files.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.text_edit_singleline(&mut move_rule.pattern);
                                    ui.label("➡");
                                    ui.text_edit_singleline(&mut move_rule.target);
                                    if ui.button("🗑").clicked() {
                                        remove_idx = Some(i);
                                    }
                                });
                            }
                            if let Some(idx) = remove_idx {
                                rule.actions.move_files.remove(idx);
                            }
                            if ui.button("➕ Add Move Rule").clicked() {
                                rule.actions.move_files.push(MoveFileRule {
                                    pattern: "".to_string(),
                                    target: "".to_string(),
                                });
                            }

                            ui.separator();
                            ui.horizontal(|ui| {
                                if ui.button("Save").clicked() {
                                    // Save to DB
                                    let st = app_state.lock();
                                    if let Some(p) = &st.db_paths {
                                        if let Ok(cfg_db) = config_db::ConfigDb::open(&p.config_db)
                                        {
                                            let _ = config_db::save_org_rule(
                                                &cfg_db.into_sqlite_db(),
                                                rule,
                                            );
                                        }
                                    }
                                    state.show_editor = false;
                                    // Refresh list
                                    if let Some(p) = &st.db_paths {
                                        if let Ok(cfg_db) = config_db::ConfigDb::open(&p.config_db)
                                        {
                                            if let Ok(rules) =
                                                config_db::list_org_rules(&cfg_db.into_sqlite_db())
                                            {
                                                state.rules = rules;
                                            }
                                        }
                                    }
                                }
                                if ui.button("Cancel").clicked() {
                                    state.show_editor = false;
                                }
                            });
                        });
                    });
            }
        }
    });
}
