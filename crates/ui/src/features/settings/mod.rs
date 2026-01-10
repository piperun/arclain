// Settings feature module

pub mod actions;
pub mod types;

// pub mod ui; // Removed in favor of views::Refactored
pub mod views;

// Re-export moved modules to maintain API compatibility
pub use views::header_config;
pub use views::pages;
pub use views::settings_content;
pub use views::settings_page;

// Re-export commonly used types
pub use views::SettingsFeature;
