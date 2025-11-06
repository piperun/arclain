// Feature modules for UI components
pub mod dialogs;
pub mod file_list;
pub mod header;
pub mod password_rules_page;
pub mod properties_panel;
pub mod settings_content;
pub mod settings_page;
pub mod status_bar;
pub mod theme;
pub mod toolbar;
pub mod tree_panel;

// Re-export commonly used types
pub use theme::{load_cjk_fonts, AppTheme, ThemeColors};
