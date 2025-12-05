use crate::features::plugins::types::PluginsListState;
use crate::features::plugins::plugins_page;
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
        let state = shared.app_state.lock();
        let plugin_manager = state.plugin_manager.as_ref();

        // Render the plugins page
        plugins_page::render(ctx, &shared.theme, &mut self.list_state, plugin_manager);
    }
}
