use crate::core::signals::AppSignals;
use crate::core::AppState;
use crate::platform::detect_dark_mode;
use crate::shared::image_assets::ImageAssetStore;
use crate::shared::theme::{load_cjk_fonts, AppTheme};
use arclain_widgets::Toaster;
use parking_lot::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Clone)]
pub struct SharedState {
    pub app_state: Arc<Mutex<AppState>>,
    /// Read-only services container (Runtime, HTTP, Plugins, etc.)
    pub services: Arc<crate::core::services::Services>,
    pub theme: AppTheme,
    pub toaster: Arc<Mutex<Toaster>>,
    /// Whether a plugin requested layout invalidation since the last frame.
    pub refresh_requests: Arc<AtomicBool>,
    pub plugin_ui_jobs: crate::features::plugins::application::PluginUiJobs,
    /// Shared cached-image state machine used by every image renderer.
    pub image_assets: ImageAssetStore,
    /// Direct access to signals without locking AppState
    pub signals: AppSignals,
    /// The application facade: owns the Tokio runtime and every composed
    /// headless service `app_state`/`services` above were populated
    /// from at startup (see `AppState::new`). Not yet read by any call
    /// site in this crate -- later Stage 1 tasks migrate `app_state`/
    /// `services` readers onto this facade's own async operation
    /// methods incrementally, retiring the fields above as they go.
    ///
    /// `Option` rather than a bare `ArclainApp`: several test fixtures
    /// (`crates/ui/tests/common/mod.rs`, `settings_controller.rs`'s own
    /// tests) build a `SharedState` by hand, deliberately skipping a
    /// full `ArclainApp::bootstrap` (directory creation, real SQLite
    /// databases, a plugin manager) to keep dispatcher-focused unit
    /// tests fast and isolated. `SharedState::new` (the real egui
    /// startup path) always sets this to `Some`.
    pub facade: Option<arclain_app::ArclainApp>,
    /// Which tab a given in-flight facade operation (archive-open,
    /// extraction, materialize) belongs to -- populated by whichever call
    /// site starts one, read by `crate::core::operation_bridge`'s
    /// background worker on every event. See that module's own doc comment
    /// for the full bridge design.
    pub operation_origins: crate::core::operation_bridge::OperationOrigins,
    /// What to do once a given in-flight `Materialize` operation completes
    /// -- see `crate::core::operation_bridge::MaterializationActions`'s own
    /// doc comment for why this exists alongside (not instead of)
    /// `operation_origins`.
    pub materialization_actions: crate::core::operation_bridge::MaterializationActions,
    /// Materialization leases currently backing a launched external
    /// application, renewed periodically for as long as this session
    /// runs -- see `crate::core::operation_bridge::ExternalOpenLeases`'s
    /// own doc comment.
    pub external_open_leases: crate::core::operation_bridge::ExternalOpenLeases,
}

impl SharedState {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);

        // Load CJK fonts during initialization
        load_cjk_fonts(&cc.egui_ctx);

        let (app_state_inner, services, facade) =
            AppState::new().expect("Failed to initialize app state");

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
                    // 0 is never a real `ArchiveSessionId` (the facade
                    // mints ids from 1) -- a safe "no session open in
                    // this tab" sentinel for this fallback context.
                    archive_session_id: tab
                        .archive_session_id
                        .get()
                        .map(arclain_app::ids::ArchiveSessionId::into_raw)
                        .unwrap_or(0),
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
            refresh_requests: Arc::new(AtomicBool::new(false)),
            plugin_ui_jobs,
            image_assets,
            signals: signals.clone(),
            facade: Some(facade),
            operation_origins: crate::core::operation_bridge::OperationOrigins::new(),
            materialization_actions: crate::core::operation_bridge::MaterializationActions::new(),
            external_open_leases: crate::core::operation_bridge::ExternalOpenLeases::new(),
        };
        // Spawns the background worker that drives archive-open/
        // extraction progress, challenges, and completion onto this
        // `SharedState`'s signals -- see `crate::core::operation_bridge`.
        crate::core::operation_bridge::spawn(&shared);

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
        crate::core::app_lifecycle::restore_tabs_on_launch(&shared);

        shared
    }

    /// Get signals without locking AppState
    /// Use this for read-only signal access instead of `app_state.lock().signals`
    #[inline]
    pub fn signals(&self) -> &AppSignals {
        &self.signals
    }
}
