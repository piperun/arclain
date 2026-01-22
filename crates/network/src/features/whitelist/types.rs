//! Types for domain whitelist management

use serde::{Deserialize, Serialize};

/// A whitelist entry for a plugin's domain access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    /// The plugin that requested this domain
    pub plugin_id: String,
    /// The domain (e.g., "dlsite.com")
    pub domain: String,
    /// Whether the user has approved this domain
    pub approved: bool,
}

impl WhitelistEntry {
    /// Create a new pending (unapproved) entry
    pub fn pending(plugin_id: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            domain: domain.into(),
            approved: false,
        }
    }

    /// Create a new approved entry
    pub fn approved(plugin_id: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            domain: domain.into(),
            approved: true,
        }
    }
}

/// Result of checking domain access
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessCheck {
    /// Domain is approved for this plugin
    Allowed,
    /// Domain exists but is not yet approved (needs user confirmation)
    NeedsApproval,
    /// Domain is not in whitelist at all
    NotWhitelisted,
}
