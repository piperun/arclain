// Core application infrastructure module

pub mod app_coordinator;
pub mod app_dialogs;
pub mod app_lifecycle;
pub mod app_rendering;
pub mod arclain_app;
pub mod navigation;
pub mod operations;
pub mod signals;
pub mod state;
pub mod utils;

// Re-export main app type
pub use navigation::{AppPage, SettingsPage};
pub use state::AppState;
