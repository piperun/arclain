// Feature modules for UI components
pub mod dialogs;
pub mod file_list;
pub mod header;
pub mod properties_panel;
pub mod status_bar;
pub mod theme;
pub mod toolbar;
pub mod tree_panel;

// Re-export commonly used types
pub use theme::{AppTheme, ThemeColors, load_cjk_fonts};
