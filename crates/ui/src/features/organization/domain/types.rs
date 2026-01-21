//! Organization feature types
//!
//! Domain types for the organization feature.

/// Actions that can be triggered from the organization UI
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrganizationAction {
    /// No action
    None,
    /// Apply organization rules
    Apply,
    /// Open the rules management UI
    ManageRules,
}

impl Default for OrganizationAction {
    fn default() -> Self {
        Self::None
    }
}
