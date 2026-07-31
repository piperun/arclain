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
//! construction, supplying the facade's one `AppSignals`-shaped fallback
//! closure, and loading persisted UI state into signals. Plugin runtime
//! wiring is application-owned; the fallback is used only when no
//! archive session is active.

use super::config_ops::describe_facade_error;
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
            encrypted_crc_policy: legacy.encrypted_crc_policy,
            db_paths: legacy.db_paths,
            dbs: legacy.dbs,
            signals: AppSignals::new(),
        };

        me.signals
            .plugin_visibility
            .set(me.user_config.plugin_visibility.clone());

        // Active context: `ArclainApp::install_active_tab_bridge` resolves
        // everything through this application's own archive-session
        // state (kept in sync with egui's active tab via
        // `crate::core::app_lifecycle::sync_active_archive_session`,
        // called once per frame). The one piece that state cannot
        // resolve on its own -- a panel-driven metadata emit with no
        // archive session active at all -- is supplied here as a plain
        // closure over `AppSignals`, since "which UI element is the
        // active tab, and how do I write to it" is exactly the one thing
        // `arclain_app` must never know about directly. See
        // `arclain_app`'s bridge documentation for why the split is
        // drawn exactly here. The facade owns the plugin runtime and
        // updates every already-loaded instance itself.
        let fallback_signals = me.signals.clone();
        facade
            .install_active_tab_bridge(move |metadata| {
                fallback_signals.tabs.get().active().metadata.set(metadata);
            })
            .map_err(|error| {
                anyhow::anyhow!("Failed to install the active-tab bridge: {error:?}")
            })?;

        let services = crate::core::services::Services {
            core: (*legacy.core_services).clone(),
        };

        // Seed the reactive settings mirrors before anything can read
        // them.
        //
        // Fatal, not best-effort: the placeholder these signals hold
        // until this call lands is not a neutral "unknown" -- it is a
        // set of concrete preference values, and acting on them is
        // destructive. `restore_tabs_on_launch` reads `false` there, so
        // a swallowed failure would both skip restoring the previous
        // session and *delete* its saved list on the way out (see
        // `app_lifecycle::save_tabs_on_exit_to`). Failing to start is
        // recoverable; silently discarding the user's session is not.
        me.refresh_settings_signals(&facade, &services.tokio_runtime)?;

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

        // Seed the canonical chrome-item signals from the application.
        me.reload_ui_config(&facade, &services.tokio_runtime);

        // Seed the chrome display options the rest of the app reads
        // through signals rather than by asking again.
        //
        // Best-effort, unlike the settings mirrors above: the placeholder
        // these two consumers hold is `UiPreferences`/`ToolbarState`'s own
        // default (labels off, both panels open), which is the same answer
        // a fresh profile gives and which nothing acts on destructively.
        // A failed read costs the user their panel-visibility preference
        // for this session, not their data -- so it is logged rather than
        // fatal.
        match services.tokio_runtime.block_on(facade.ui_display_options()) {
            Ok(options) => {
                let mut prefs = me.signals.ui_preferences.get();
                prefs.show_button_labels = options.show_button_labels;
                me.signals.ui_preferences.set(prefs);

                let tab = me.signals.tabs.get().active().clone();
                let mut view_state = tab.browser_view_state.get();
                view_state.toolbar_state.show_tree_panel = options.tree_panel_visible;
                view_state.toolbar_state.show_properties_panel = options.properties_panel_visible;
                tab.browser_view_state.set(view_state);
            }
            Err(error) => {
                tracing::warn!(
                    "{}",
                    describe_facade_error("reading the interface display options", error)
                );
            }
        }

        Ok((me, services, facade))
    }
}
