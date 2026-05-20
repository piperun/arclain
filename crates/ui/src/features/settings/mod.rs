pub mod application;
pub mod domain;
pub mod presentation;

pub use presentation::{SettingsFeature, SettingsFeatureBorrows, SettingsFeatureRefs};

// Re-exports for compatibility and internal convenience
pub use domain::types;
pub use presentation::pages;
pub use presentation::views;
pub use presentation::views::header_config;
pub use presentation::views::settings_content;
pub use presentation::views::settings_page;

pub mod actions {
    pub use crate::features::settings::presentation::controllers::settings_controller::*;
}
