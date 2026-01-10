// Archive browser feature module

pub mod actions;
pub mod feature;
pub mod navigation;

pub use feature::ArchiveBrowser;
pub mod state;
pub mod types;
pub mod views;

// Re-exports
pub use views::browser;

pub use actions::ActionContext;
pub use actions::ArchiveBrowserAction;
pub use state::ArchiveBrowserState;
// pub use ui::render_archive_browser; // Changed to browser::render_archive_browser.

pub use browser::render_archive_browser;
