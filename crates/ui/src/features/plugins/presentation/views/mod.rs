pub mod detail_view;
pub mod install_dialog;
pub mod list_view;
pub mod plugin_list;
pub mod rendering;
pub mod settings_view;

pub use install_dialog::{render as render_plugin_install_dialog, PluginInstallDialogResult};
pub use settings_view::render as render_plugins_settings;

// Re-export UI components for backward compatibility/rendering engine use
pub use crate::features::plugins::presentation::rendering as ui;
