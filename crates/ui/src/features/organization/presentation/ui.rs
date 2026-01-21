use crate::features::organization::OrganizerPage;
use crate::features::settings::pages::RulesPage;

use crate::shared::SharedState;
use eframe::egui;

pub enum OrganizationAction {
    None,
    Apply,
    ManageRules,
}

pub struct OrganizationFeature {
    pub organizer_page: Option<OrganizerPage>,
    pub rules_page: RulesPage,
}

impl OrganizationFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            organizer_page: None,
            rules_page: RulesPage::new(),
        }
    }

    // ensure_rules_loaded removed as RulesPage handles it internally

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> OrganizationAction {
        let mut action = OrganizationAction::None;

        if let Some(page) = &mut self.organizer_page {
            if let Some(result) = page.render(ctx, &shared.theme) {
                match result {
                    crate::features::organization::OrganizePanelAction::Apply => {
                        action = OrganizationAction::Apply;
                    }
                    crate::features::organization::OrganizePanelAction::ManageRules => {
                        action = OrganizationAction::ManageRules;
                    }
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
