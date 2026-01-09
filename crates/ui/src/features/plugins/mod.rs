// Plugins feature module

pub mod actions;
pub mod dialog_state;
pub mod plugin_list;
pub mod plugins_page;
pub mod rendering;
pub mod types;
pub mod views;

pub mod ui;

// Re-export commonly used types
pub use dialog_state::PluginDialogState;
pub use rendering::{render_dialog, render_page};
pub use ui::PluginsFeature;
