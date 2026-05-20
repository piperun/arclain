//! Configuration sync and reload operations

use super::AppState;
use tracing::warn;

impl AppState {
    /// Synchronize configuration (rules, filters) from TOML defaults to DB
    pub fn sync_configuration(&self) {
        if let Some(ref dbs) = self.dbs {
            let config_pool = &dbs.config_pool;
            if let Err(e) = arclain_core::config::sync::sync_rules(config_pool) {
                warn!("Failed to sync organization rules: {}", e);
            }
            // Title filters: init() seeds system replacements and
            // primes the in-memory cache. Goes through the shared
            // Diesel pool, not a fresh ConfigDb handle.
            if let Err(e) = arclain_core::utilities::title_filter::init(config_pool) {
                warn!("Failed to initialize title filters: {}", e);
            }
        }
    }

    /// Refresh UI configuration (toolbar/info panel items) from UiService
    pub fn reload_ui_config(&mut self, ui_service: &arclain_core::UiService) {
        if let Ok(items) = ui_service.list_toolbar_items() {
            self.signals.toolbar_items.set(items);
        }
        if let Ok(items) = ui_service.list_info_panel_items() {
            self.signals.info_panel_items.set(items);
        }
    }
}
