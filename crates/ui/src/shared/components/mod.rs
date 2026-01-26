// Shared reusable UI components module

pub mod breadcrumbs;
pub mod context_menu;
pub mod header;
pub mod item_table;
pub mod layout;
pub mod network_log;
pub mod panel;
pub mod preview_tree;
pub mod properties_panel;
pub mod search_bar;
pub mod settings_form;
pub mod settings_header;
pub mod status_bar;
pub mod status_icon;
pub mod switch;
pub mod toolbar;
pub mod variables_panel;
pub mod top_tab_bar;
pub mod tree_panel;

pub use breadcrumbs::Breadcrumbs;
pub use layout::{Center, Column, CrossAxisAlignment, FormField, MainAxisAlignment, Padding, Row, Section, SizedBox, Spacer};
pub use search_bar::SearchBar;
pub use settings_form::Form;
pub use settings_header::SettingsHeader;
pub use switch::Switch;
pub use variables_panel::{TemplateVariable, VariableGroup, VariablePicker, VariablesPanel};

// Re-export commonly used types and states
// pub use file_list::{FileEntry, FileListAction, SortState};
pub use header::HeaderState;
pub use properties_panel::PropertiesPanelAction;
pub use status_bar::StatusBarInfo;
// pub use status_bar::PluginStatusInfo;
// pub use toolbar::ToolbarState;
// pub use tree_panel::TreePanelState;
