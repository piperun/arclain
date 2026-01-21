//! Settings page components
//!
//! This module previously contained navigation components.
//! They have been moved to navigation.rs.

// Re-export navigation components for backward compatibility
pub use super::navigation::{
    render_breadcrumb, render_settings_navigator, render_settings_overview,
    render_settings_search_results,
};
