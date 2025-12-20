use crate::core::AppState;
use crate::features::plugins::PluginDialogState;
use crate::platform::detect_dark_mode;
use crate::shared::theme::{load_cjk_fonts, AppTheme};
use arclain_widgets::Toaster;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone)]
pub struct SharedState {
    pub app_state: Arc<Mutex<AppState>>,
    pub theme: AppTheme,
    pub toaster: Arc<Mutex<Toaster>>,
    #[allow(dead_code)] // Future use for dialog rendering
    pub plugin_dialog_state: Arc<Mutex<PluginDialogState>>,
    /// Panel refresh requests from plugins (extension point names to refresh)
    pub refresh_requests: Arc<Mutex<Vec<String>>>,
}

impl SharedState {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark_mode = detect_dark_mode();
        let theme = AppTheme::new(dark_mode);

        // Load CJK fonts during initialization
        load_cjk_fonts(&cc.egui_ctx);

        let app_state = Arc::new(Mutex::new(
            AppState::new().expect("Failed to initialize app state"),
        ));

        Self {
            app_state,
            theme,
            toaster: Arc::new(Mutex::new(Toaster::new())),
            plugin_dialog_state: Arc::new(Mutex::new(PluginDialogState::new())),
            refresh_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
