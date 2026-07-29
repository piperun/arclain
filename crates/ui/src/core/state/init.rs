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
//! construction, installing the plugin manager's active-tab bridge (the
//! facade builds the bridge itself; this only supplies the one
//! `AppSignals`-shaped fallback closure it cannot build on its own -- see
//! the install call's own comment), and loading persisted UI state into
//! signals.

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
            signals: AppSignals::new(),
        };

        me.signals.user_config.set(me.user_config.clone());
        me.signals.pass_rules.set(me.pass_rules.clone());

        // Active context: `ArclainApp::active_tab_bridge` resolves
        // everything through this application's own archive-session
        // state (kept in sync with egui's active tab via
        // `crate::core::app_lifecycle::sync_active_archive_session`,
        // called once per frame). The one piece that state cannot
        // resolve on its own -- a panel-driven metadata emit with no
        // archive session active at all -- is supplied here as a plain
        // closure over `AppSignals`, since "which UI element is the
        // active tab, and how do I write to it" is exactly the one thing
        // `arclain_app` must never know about directly. See
        // `arclain_app::plugins::ProductionActiveTabBridge`'s own doc
        // comment for why the split is drawn exactly here.
        if let Some(ref plugin_manager) = legacy.plugin_manager {
            let fallback_signals = me.signals.clone();
            let bridge = facade.active_tab_bridge(move |metadata| {
                fallback_signals.tabs.get().active().metadata.set(metadata);
            });
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
