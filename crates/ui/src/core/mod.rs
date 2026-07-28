// Core application infrastructure module

pub mod app_lifecycle;
pub mod app_rendering;
pub mod arclain_app;
pub mod file_drop;
pub mod navigation;
pub mod operation_bridge;
pub mod operations;
pub mod services;
pub mod signals;
pub mod state;
pub mod tabs;
pub mod utils;

// Re-export main app type
pub use navigation::{AppPage, SettingsPage};
pub use state::AppState;
