//! Archive Browser feature
//!
//! Provides hierarchical browsing of archive contents with list and grid views,
//! breadcrumb navigation, and integration with archive operations.

pub mod application;
pub mod domain;
pub mod presentation;

mod feature;

// Re-exports for easier access
pub use domain::types::{
    Action, Action as ArchiveBrowserAction, BrowserViewState as ArchiveBrowserState,
};
pub use feature::ArchiveBrowser;
pub use presentation::controllers::browser_controller::BrowserController;
