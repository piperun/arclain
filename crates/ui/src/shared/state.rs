use crate::core::AppState;
use crate::platform::detect_dark_mode;
use crate::shared::theme::{load_cjk_fonts, AppTheme};
use parking_lot::Mutex;
use std::sync::Arc;

pub struct SharedState {
    pub app_state: Arc<Mutex<AppState>>,
    pub theme: AppTheme,
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

        Self { app_state, theme }
    }
}
