//! Settings Pages Module
//!
//! Contains individual settings page implementations.

pub mod interface;
pub mod organization_rules;

// Re-export for convenience
pub use interface::render_interface_settings;
pub use organization_rules::RulesPage;
