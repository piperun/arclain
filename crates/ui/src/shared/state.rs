use crate::core::signals::AppSignals;
use crate::core::AppState;
use crate::platform::detect_dark_mode;
use crate::shared::theme::{load_cjk_fonts, AppTheme};
use arclain_plugins::types::PluginAction;
use arclain_widgets::Toaster;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone)]
pub struct SharedState {
    pub app_state: Arc<Mutex<AppState>>,
    /// Read-only services container (Runtime, HTTP, Plugins, etc.)
    pub services: Arc<crate::core::services::Services>,
    pub theme: AppTheme,
    pub toaster: Arc<Mutex<Toaster>>,
    /// Panel refresh requests from plugins (extension point names to refresh)
    pub refresh_requests: Arc<Mutex<Vec<String>>>,
    /// Plugin actions emitted from background `send_ui_event` threads
    /// (the dispatch helper pushes here). Drained by detail_view at the
    /// start of each render so actions reach `process_plugin_actions`.
    /// Lives on SharedState (not PluginsListState) so every dispatcher —
    /// detail view, dialog/page callbacks, toolbar, panel — can write
    /// to one shared sink.
    pub pending_plugin_actions: Arc<Mutex<Vec<(String, PluginAction)>>>,
    /// Direct access to signals without locking AppState
    pub signals: AppSignals,
}

impl SharedState {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);

        // Load CJK fonts during initialization
        load_cjk_fonts(&cc.egui_ctx);

        let (app_state_inner, services) = AppState::new().expect("Failed to initialize app state");

        // Clone signals before wrapping in Arc<Mutex>
        let signals = app_state_inner.signals.clone();

        let app_state = Arc::new(Mutex::new(app_state_inner));
        let services = Arc::new(services);

        let shared = Self {
            app_state: app_state.clone(),
            services,
            theme,
            toaster: Arc::new(Mutex::new(Toaster::new())),
            refresh_requests: Arc::new(Mutex::new(Vec::new())),
            pending_plugin_actions: Arc::new(Mutex::new(Vec::new())),
            signals: signals.clone(),
        };

        // One-shot upgrade of legacy auto-saved password rules.
        //
        // Rules auto-saved before the smart-pattern heuristic carry a
        // `regex::escape(filename)` pattern that matches exactly one
        // archive, so siblings sharing the same DLsite code / maker
        // bracket would re-prompt for a password. `upgrade_auto_saved_rules`
        // re-derives the broader pattern for ONLY those provably-narrow
        // auto-saved rules (fingerprint: `"Auto-saved: <file>"` name +
        // literal-escape pattern), leaving hand-edited rules untouched.
        // Runs before tab restore so any archive reopened below benefits
        // from the broadened patterns immediately. Idempotent — a no-op
        // on every launch after the first.
        {
            let mut st = app_state.lock();
            if let Some(upgraded) =
                arclain_core::utilities::password_matcher::upgrade_auto_saved_rules(&st.pass_rules)
            {
                let changed = upgraded
                    .iter()
                    .zip(&st.pass_rules)
                    .filter(|(new, old)| new.pattern != old.pattern)
                    .count();
                // Persists (re-encrypts the whole set into the secrets
                // DB) and updates the in-memory cache; refresh the
                // lock-free signal mirror to match.
                let _ = st.save_password_rules(upgraded);
                let mirror = st.pass_rules.clone();
                st.signals.pass_rules.set(mirror);
                drop(st);
                if changed > 0 {
                    shared.toaster.lock().success(format!(
                        "Upgraded {changed} saved password rule{} to match sibling archives",
                        if changed == 1 { "" } else { "s" }
                    ));
                }
            }
        }

        // Restore previous tab session if the setting is enabled.
        crate::core::app_lifecycle::restore_tabs_on_launch(&app_state, &signals);

        shared
    }

    /// Get signals without locking AppState
    /// Use this for read-only signal access instead of `app_state.lock().signals`
    #[inline]
    pub fn signals(&self) -> &AppSignals {
        &self.signals
    }
}
