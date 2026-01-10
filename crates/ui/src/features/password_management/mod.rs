// Password management feature

pub mod actions;
pub mod operations;
pub mod ui;
pub mod views;

// Re-export moved modules
pub use views::dialogs;
pub use views::rules_page;

// Re-export commonly used types
pub use operations::{PasswordFeature, PasswordFeatureAction};
pub use ui::handle_password_dialogs;
