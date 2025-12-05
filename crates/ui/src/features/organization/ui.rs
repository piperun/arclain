use crate::features::organization::organize_panel::OrganizePanel;
use crate::features::organization::rules_page::OrganizationRulesState;
use crate::shared::SharedState;
use eframe::egui;

pub enum OrganizationAction {
    None,
    Apply,
    Cancel,
}

pub struct OrganizationFeature {
    pub organize_panel: Option<OrganizePanel>,
    pub rules_state: OrganizationRulesState,
}

impl OrganizationFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            organize_panel: None,
            rules_state: OrganizationRulesState::default(),
        }
    }

    pub fn ensure_rules_loaded(
        &mut self,
        app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    ) {
        if self.rules_state.rules.is_empty() {
            let st = app_state.lock();
            if let Some(p) = &st.db_paths {
                if let Ok(cfg_db) = arclain_core::config::database::ConfigDb::open(&p.config_db) {
                    if let Ok(rules) =
                        arclain_core::config::database::list_org_rules(&cfg_db.into_sqlite_db())
                    {
                        self.rules_state.rules = rules;
                    }
                }
            }
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, _shared: &SharedState) -> OrganizationAction {
        let mut action = OrganizationAction::None;

        if let Some(panel) = &mut self.organize_panel {
            if let Some(result) = panel.render(ctx) {
                if result {
                    action = OrganizationAction::Apply;
                } else {
                    action = OrganizationAction::Cancel;
                }
            }
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("No active organization session.");
            });
        }

        action
    }
}
