use crate::core::signals::AppSignals;
use crate::core::AppState;
use crate::platform::detect_dark_mode;
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

        Self {
            app_state,
            services,
            theme,
            toaster: Arc::new(Mutex::new(Toaster::new())),
            refresh_requests: Arc::new(Mutex::new(Vec::new())),
            signals,
        }
    }

    /// Get signals without locking AppState
    /// Use this for read-only signal access instead of `app_state.lock().signals`
    #[inline]
    pub fn signals(&self) -> &AppSignals {
        &self.signals
    }
}
