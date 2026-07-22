use crate::core::signals::AppSignals;
use crate::core::AppState;
use crate::platform::detect_dark_mode;
use crate::shared::image_assets::ImageAssetStore;
use crate::shared::theme::{load_cjk_fonts, AppTheme};
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
    pub plugin_ui_jobs: crate::features::plugins::application::PluginUiJobs,
    /// Shared cached-image state machine used by every image renderer.
    pub image_assets: ImageAssetStore,
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

        let plugin_ui_jobs = crate::features::plugins::application::PluginUiJobs::new(
            services.plugin_manager.clone(),
            services.tokio_runtime.clone(),
        )
        .with_origin_context_provider({
            let signals = signals.clone();
            move |tab_id| {
                let tabs = signals.tabs.get();
                let tab = tabs.get(tab_id)?.clone();
                drop(tabs);
                Some(arclain_plugins::host_functions::EventContext {
                    archive_path: tab
                        .archive_path
                        .get()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    password: tab.current_password.get(),
                    entries: tab.entries.get(),
                    metadata_signal: tab.metadata.clone(),
                })
            }
        });
        let image_assets = services.content_cache.as_ref().map_or_else(
            || ImageAssetStore::without_cache(services.tokio_runtime.clone()),
            |cache| ImageAssetStore::new(cache.clone(), services.tokio_runtime.clone()),
        );
        let shared = Self {
            app_state: app_state.clone(),
            services,
            theme,
            toaster: Arc::new(Mutex::new(Toaster::new())),
            refresh_requests: Arc::new(Mutex::new(Vec::new())),
            plugin_ui_jobs,
            image_assets,
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
