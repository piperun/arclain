use super::organize_panel::{OrganizePanel, OrganizePanelAction};
use crate::shared::theme::AppTheme;
use eframe::egui;

pub struct OrganizerPage {
    pub panel: OrganizePanel,
}

impl OrganizerPage {
    pub fn new(panel: OrganizePanel) -> Self {
        Self { panel }
    }

    pub fn render(
        &mut self,
        ctx: &egui::Context,
        _theme: &AppTheme,
    ) -> Option<OrganizePanelAction> {
        let mut action = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::NONE.inner_margin(16.0).show(ui, |ui| {
                action = self.panel.render(ui, ctx, _theme);
            });
        });
        action
    }
}
