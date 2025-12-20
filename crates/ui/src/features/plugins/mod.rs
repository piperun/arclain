// Plugins feature module

pub mod action_handler;
pub mod dialog_state;
pub mod plugin_list;
pub mod plugin_ui;
pub mod plugins_page;
pub mod types;
pub mod views;

pub mod ui;

// Re-export commonly used types
pub use dialog_state::PluginDialogState;
pub use ui::PluginsFeature;
