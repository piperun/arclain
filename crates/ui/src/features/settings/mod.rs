// Settings feature module

pub mod actions;
pub mod header_config;
pub mod pages;
pub mod settings_content;
pub mod settings_page;
pub mod types;

// pub mod ui; // Removed in favor of views::Refactored
pub mod views;

// Re-export commonly used types
pub use views::SettingsFeature;
