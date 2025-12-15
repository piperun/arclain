use crate::features::plugins::plugins_page;
use crate::features::plugins::types::PluginsListState;
use crate::shared::SharedState;
use eframe::egui;

pub struct PluginsFeature {
    pub list_state: PluginsListState,
}

impl PluginsFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            list_state: PluginsListState::default(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) {
        let manager_arc = shared.app_state.lock().plugin_manager.clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            let guard = manager_arc.as_ref().map(|m| m.lock());
            let manager_ref = guard.as_deref();

            plugins_page::render(
                ui,
                &shared.theme,
                manager_ref,
                &mut self.list_state,
                &shared.app_state,
            );
        });
    }
}
