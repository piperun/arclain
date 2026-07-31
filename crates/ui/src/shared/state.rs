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
    pub plugin_ui_jobs: crate::features::plugins::application::PluginUiJobs,
    /// Plugin UI slots served by the application facade's session
    /// contract -- see
    /// `crate::features::plugins::application::facade_sessions`.
    pub plugin_sessions: crate::features::plugins::application::PluginSessions,
    /// Shared cached-image state machine used by every image renderer.
    pub image_assets: ImageAssetStore,
    /// Direct access to signals without locking AppState
    pub signals: AppSignals,
    /// The application facade: owns the Tokio runtime and every composed
    /// headless service `app_state`/`services` above were populated
    /// from at startup (see `AppState::new`). Plugin UI and the migrated
    /// archive workflows read through this handle; later Stage 1 tasks
    /// retire the remaining `app_state`/`services` readers incrementally.
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
            Some(facade.clone()),
            services.tokio_runtime.clone(),
        );
        // Every image reference resolves through the application: a plugin
        // document's keys are facade-encoded and namespaced to the owning
        // plugin (a bare key is ambiguous once it leaves its session), and
        // every other key belongs to the host namespace. The facade decodes
        // which is which, enforces its own per-asset cap on both, and owns
        // the URL-fallback fetch -- so this frontend holds no cache handle
        // and no HTTP client of its own.
        let image_assets = ImageAssetStore::new(facade.clone(), services.tokio_runtime.clone());
        let shared = Self {
            app_state: app_state.clone(),
            services,
            theme,
            toaster: Arc::new(Mutex::new(Toaster::new())),
            plugin_ui_jobs,
            plugin_sessions: crate::features::plugins::application::PluginSessions::new(),
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

        // Bootstrap broadens legacy auto-saved password rules that still
        // match exactly one archive (see `ArclainApp::
        // startup_password_rule_upgrades`). It happens behind the facade,
        // before anything can read a rule; all that is left here is
        // telling the user their saved rules changed under them.
        if let Some(ref facade) = shared.facade {
            let changed = facade.startup_password_rule_upgrades();
            if changed > 0 {
                shared.toaster.lock().success(format!(
                    "Upgraded {changed} saved password rule{} to match sibling archives",
                    if changed == 1 { "" } else { "s" }
                ));
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
