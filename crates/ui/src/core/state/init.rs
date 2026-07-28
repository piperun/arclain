//! AppState initialization
//!
//! `AppState::new` used to perform the entire application composition
//! sequence directly (directories, configuration, databases, backends,
//! plugins, ...). That sequence now lives in `arclain_app::ArclainApp::
//! bootstrap`, which this function calls and then unpacks via
//! `take_legacy_composition` into this crate's still-existing
//! `AppState`/`Services` shapes -- unmigrated call sites elsewhere in
//! this crate keep reading `shared_state.app_state`/`shared_state.services`
//! exactly as before. What's left here is genuinely UI-only: `AppSignals`
//! construction, wiring the live per-tab plugin bridge (which needs
//! `AppSignals`, so `arclain_app` cannot build it), and loading persisted
//! UI state into signals.

use super::AppState;
use crate::core::signals::AppSignals;
use anyhow::Result;
use tracing::info;

impl AppState {
    pub fn new() -> Result<(
        Self,
        crate::core::services::Services,
        arclain_app::ArclainApp,
    )> {
        info!("Initializing application state");

        let facade =
            arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig::system_default())
                .map_err(|error| anyhow::anyhow!("Failed to bootstrap application: {error:?}"))?;
        let legacy = facade
            .take_legacy_composition()
            .map_err(|error| anyhow::anyhow!("Failed to take legacy composition: {error:?}"))?;

        let me = Self {
            user_config: legacy.user_config.clone(),
            pass_rules: legacy.pass_rules.clone(),
            backend_selector: legacy.backend_selector,
            fallback_backend: legacy.fallback_backend,
            last_entries: vec![],
            encrypted_crc_policy: legacy.encrypted_crc_policy,
            db_paths: legacy.db_paths,
            dbs: legacy.dbs,
            plugin_event_scheduler: legacy.plugin_event_scheduler,
            pending_plugin_events: Vec::new(),
            signals: AppSignals::new(),
        };

        me.signals.user_config.set(me.user_config.clone());
        me.signals.pass_rules.set(me.pass_rules.clone());

        // Active context: the one composition step that must happen
        // here rather than inside `ArclainApp::bootstrap`, because it
        // needs `AppSignals` -- an egui-integration type `arclain_app`
        // must never depend on. Resolves through `AppSignals` at call
        // time so plugins always see the currently active tab; see
        // `arclain_plugins::active_tab` for the design rationale.
        if let Some(ref plugin_manager) = legacy.plugin_manager {
            let bridge = std::sync::Arc::new(
                crate::shared::active_tab_bridge::AppSignalsBridge::new(me.signals.clone()),
            );
            plugin_manager.lock().set_active_tab_bridge(bridge);
        }

        let services = crate::core::services::Services {
            core: (*legacy.core_services).clone(),
            plugin_manager: legacy.plugin_manager,
            content_cache: legacy.content_cache,
            resource_manager: legacy.resource_manager,
        };

        // Set initial server connection status signal based on startup health check.
        // GametaClient caches the version from the health check performed in
        // init_db_services, so no second network call is needed here.
        if let Some(ref gc) = services.core.gameta_client {
            let version = gc
                .last_known_version()
                .unwrap_or_else(|| "unknown".to_string());
            me.signals
                .server_status
                .set(crate::core::signals::ServerConnectionStatus::Connected(
                    version,
                ));
        } else if me.user_config.gameta_server_enabled {
            // Enabled in config but client wasn't created (health check failed or no URL)
            me.signals
                .server_status
                .set(crate::core::signals::ServerConnectionStatus::Error(
                    "Connection failed at startup".to_string(),
                ));
        }
        // If gameta_server_enabled is false, server_status stays Offline (default)

        // Load UI items via UiService now that services are ready
        if let Some(ref svc) = services.ui_service {
            if let Ok(items) = svc.list_toolbar_items() {
                me.signals.toolbar_items.set(items);
            }
            if let Ok(items) = svc.list_info_panel_items() {
                me.signals.info_panel_items.set(items);
            }
            if let Ok(items) = svc.list_items(arclain_core::UiRegion::ContextMenu) {
                me.signals.context_menu_items.set(items);
            }

            // Load UI preferences from database
            if let Ok(Some(show_labels_str)) = svc.get_display_option("show_button_labels") {
                let show_labels = show_labels_str == "true";
                let mut prefs = me.signals.ui_preferences.get();
                prefs.show_button_labels = show_labels;
                me.signals.ui_preferences.set(prefs);
            }

            // Load panel defaults from database and set them in browser view state
            let tree_visible = svc
                .get_display_option("tree_panel_visible")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true);
            let properties_visible = svc
                .get_display_option("properties_panel_visible")
                .ok()
                .flatten()
                .map(|v| v == "true")
                .unwrap_or(true);

            let tab = me.signals.tabs.get().active().clone();
            let mut view_state = tab.browser_view_state.get();
            view_state.toolbar_state.show_tree_panel = tree_visible;
            view_state.toolbar_state.show_properties_panel = properties_visible;
            tab.browser_view_state.set(view_state);
        }

        Ok((me, services, facade))
    }
}
