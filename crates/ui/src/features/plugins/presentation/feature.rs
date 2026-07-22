use crate::features::plugins::domain::types::PluginsListState;
use crate::features::plugins::presentation::pages::plugins_page;
use crate::shared::SharedState;
use eframe::egui;

pub struct PluginsFeature {
    /// Standalone Plugins page list state (route: AppPage::Plugins).
    pub list_state: PluginsListState,
    /// Plugins **settings** page list state (route:
    /// AppPage::Settings(SettingsPage::Plugins)).
    ///
    /// Kept separate from `list_state` so the standalone and settings
    /// render paths retain independent selection / scroll / install
    /// progress without one page's UI state leaking into the other.
    pub settings_list_state: PluginsListState,
}

impl PluginsFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            list_state: PluginsListState::default(),
            settings_list_state: PluginsListState::default(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) {
        let content_cache = shared.services.content_cache.clone();
        crate::features::plugins::application::request_plugin_snapshot(
            shared,
            &mut self.list_state,
        );

        egui::CentralPanel::default().show(ctx, |ui| {
            plugins_page::render(
                ui,
                &shared.theme,
                &mut self.list_state,
                &shared.app_state,
                Some(shared),
                content_cache,
            );
        });
    }
}
