use crate::features::organization::presentation::views::{ProfilesPage, RulesPage};
use crate::features::organization::OrganizerPage;

use crate::shared::SharedState;
use eframe::egui;

// Re-export OrganizationAction from domain
pub use crate::features::organization::domain::types::OrganizationAction;

pub struct OrganizationFeature {
    pub organizer_page: Option<OrganizerPage>,
    pub rules_page: RulesPage,
    pub profiles_page: ProfilesPage,
}

impl OrganizationFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            organizer_page: None,
            rules_page: RulesPage::new(),
            profiles_page: ProfilesPage::new(),
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
