//! Configuration reload operations
//!
//! The startup-time sync this file used to also hold
//! (`sync_configuration`: seeding organization rules and title filters
//! from TOML defaults into the database) moved into `arclain_app::
//! runtime::bootstrap::run` -- it needed nothing from `AppState` beyond
//! `self.dbs`, which that function now has its own copy of during
//! composition. `sync_configuration`'s only caller
//! (`crates/ui/src/core/state/init.rs`) was removed along with it.

use super::AppState;

impl AppState {
    /// Refresh UI configuration (toolbar/info panel/context menu items)
    /// from UiService. Called from the settings header save handlers
    /// after a layout-editor save lands.
    pub fn reload_ui_config(&mut self, ui_service: &arclain_core::UiService) {
        if let Ok(items) = ui_service.list_toolbar_items() {
            self.signals.toolbar_items.set(items);
        }
        if let Ok(items) = ui_service.list_info_panel_items() {
            self.signals.info_panel_items.set(items);
        }
        if let Ok(items) = ui_service.list_items(arclain_core::UiRegion::ContextMenu) {
            self.signals.context_menu_items.set(items);
        }
    }
}
