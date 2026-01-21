use super::OrganizePanel;
use crate::shared::components::network_log::NetworkLog;
use eframe::egui;

impl OrganizePanel {
    pub(super) fn render_network_tab(&self, ui: &mut egui::Ui) {
        NetworkLog::render(ui, &self.session.network_log);
    }
}
