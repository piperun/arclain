// Shared reusable UI components module

use crate::shared::theme::AppTheme;
use eframe::egui;

/// Render an error-tinted label using the theme's `colors.error`.
///
/// Centralizes the pattern of `ui.colored_label(theme.colors.error, msg)`
/// that previously appeared inline at 5+ form/dialog sites with
/// hardcoded `Color32::from_rgb(220, 53, 69)` or `Color32::RED`.
pub fn error_label(ui: &mut egui::Ui, theme: &AppTheme, msg: &str) {
    ui.colored_label(theme.colors.error, msg);
}

pub mod breadcrumbs;
pub mod carousel;
pub mod drop_overlay;
pub mod header;
pub mod item_table;
pub mod layout;
pub mod logs_page;
pub mod network_log;
// `panel` and `properties_panel` moved to
// `features/archive_browser/presentation/components/` in the
// 2026-05-19 audit — they're archive_browser-specific (render
// archive properties + plugin extensions) and their `features/plugins`
// imports violated the shared/ boundary.
pub mod preview_tree;
pub mod search_bar;
pub mod search_palette;
pub mod settings_card;
pub mod settings_form;
pub mod settings_header;
pub mod status_bar;
pub mod status_icon;
pub mod tab_bar;
pub mod toolbar;
pub mod top_tab_bar;
pub mod tree_panel;
pub mod variables_panel;

pub use breadcrumbs::Breadcrumbs;
pub use layout::{
    Center, Column, CrossAxisAlignment, FormField, MainAxisAlignment, Padding, Row, Section,
    SizedBox, Spacer,
};
pub use search_bar::SearchBar;
pub use settings_card::SettingsCard;
pub use settings_form::Form;
pub use settings_header::SettingsHeader;
pub use variables_panel::{TemplateVariable, VariableGroup, VariablePicker};

// Re-export commonly used types and states
// pub use file_list::{FileEntry, FileListAction, SortState};
pub use header::HeaderState;
pub use status_bar::StatusBarInfo;
// pub use status_bar::PluginStatusInfo;
// pub use toolbar::ToolbarState;
// pub use tree_panel::TreePanelState;
