use crate::features::organization::organize_panel::OrganizePanel;
use crate::features::settings::pages::RulesPage;
use crate::shared::SharedState;
use eframe::egui;

pub enum OrganizationAction {
    None,
    Apply,
}

pub struct OrganizationFeature {
    pub organize_panel: Option<OrganizePanel>,
    pub rules_page: RulesPage,
}

impl OrganizationFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            organize_panel: None,
            rules_page: RulesPage::new(),
        }
    }

    // ensure_rules_loaded removed as RulesPage handles it internally

    pub fn render(&mut self, ctx: &egui::Context, _shared: &SharedState) -> OrganizationAction {
        let mut action = OrganizationAction::None;

        if let Some(panel) = &mut self.organize_panel {
            if let Some(result) = panel.render(ctx) {
                match result {
                    crate::features::organization::OrganizePanelAction::Apply => {
                        action = OrganizationAction::Apply;
                    }
                    _ => {}
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
