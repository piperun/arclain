// Shared reusable UI components module

pub mod breadcrumbs;
pub mod context_menu;
pub mod file_list;
pub mod header;
pub mod network_log;
pub mod preview_tree;
pub mod properties_panel;
pub mod search_bar;
pub mod settings_form;
pub mod settings_header;
pub mod status_bar;
pub mod toolbar;
pub mod tree_panel;

pub use breadcrumbs::Breadcrumbs;
pub use search_bar::SearchBar;
pub use settings_form::SettingsForm;
pub use settings_header::SettingsHeader;

// Re-export commonly used types and states
// pub use file_list::{FileEntry, FileListAction, SortState};
pub use header::HeaderState;
pub use properties_panel::PropertiesPanelAction;
pub use status_bar::StatusBarInfo;
// pub use status_bar::PluginStatusInfo;
// pub use toolbar::ToolbarState;
// pub use tree_panel::TreePanelState;
