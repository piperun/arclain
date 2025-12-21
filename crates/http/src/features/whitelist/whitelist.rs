//! Domain whitelist management
//!
//! Controls which domains each plugin can access.

use super::types::{AccessCheck, WhitelistEntry};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Manages domain access permissions for plugins
#[derive(Debug, Default)]
pub struct DomainWhitelist {
    /// plugin_id -> set of approved domains
    approved: RwLock<HashMap<String, HashSet<String>>>,
    /// plugin_id -> set of pending (unapproved) domains
    pending: RwLock<HashMap<String, HashSet<String>>>,
}

impl DomainWhitelist {
    /// Create a new empty whitelist
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a whitelist from database entries
    pub fn from_entries(entries: Vec<WhitelistEntry>) -> Self {
        let mut approved: HashMap<String, HashSet<String>> = HashMap::new();
        let mut pending: HashMap<String, HashSet<String>> = HashMap::new();

        for entry in entries {
            if entry.approved {
                approved
                    .entry(entry.plugin_id)
                    .or_default()
                    .insert(entry.domain);
            } else {
                pending
                    .entry(entry.plugin_id)
                    .or_default()
                    .insert(entry.domain);
            }
        }

        Self {
            approved: RwLock::new(approved),
            pending: RwLock::new(pending),
        }
    }

    /// Check if a domain is allowed for a plugin
    pub fn check(&self, plugin_id: &str, domain: &str) -> AccessCheck {
        let domain_normalized = normalize_domain(domain);

        // Check approved first
        let approved = self.approved.read();
        if let Some(domains) = approved.get(plugin_id) {
            if domains.contains(&domain_normalized) {
                return AccessCheck::Allowed;
            }
        }
        drop(approved);

        // Check pending
        let pending = self.pending.read();
        if let Some(domains) = pending.get(plugin_id) {
            if domains.contains(&domain_normalized) {
                return AccessCheck::NeedsApproval;
            }
        }

        AccessCheck::NotWhitelisted
    }

    /// Check if domain is allowed (simple bool version)
    pub fn is_allowed(&self, plugin_id: &str, domain: &str) -> bool {
        self.check(plugin_id, domain) == AccessCheck::Allowed
    }

    /// Add a pending domain request
    pub fn add_pending(&self, plugin_id: &str, domain: &str) {
        let domain_normalized = normalize_domain(domain);
        self.pending
            .write()
            .entry(plugin_id.to_string())
            .or_default()
            .insert(domain_normalized);
    }

    /// Approve a pending domain
    pub fn approve(&self, plugin_id: &str, domain: &str) {
        let domain_normalized = normalize_domain(domain);

        // Remove from pending
        if let Some(domains) = self.pending.write().get_mut(plugin_id) {
            domains.remove(&domain_normalized);
        }

        // Add to approved
        self.approved
            .write()
            .entry(plugin_id.to_string())
            .or_default()
            .insert(domain_normalized);
    }

    /// Revoke an approved domain
    pub fn revoke(&self, plugin_id: &str, domain: &str) {
        let domain_normalized = normalize_domain(domain);

        if let Some(domains) = self.approved.write().get_mut(plugin_id) {
            domains.remove(&domain_normalized);
        }
    }

    /// Get all pending requests for UI display
    pub fn get_pending(&self) -> Vec<WhitelistEntry> {
        let pending = self.pending.read();
        let mut entries = Vec::new();

        for (plugin_id, domains) in pending.iter() {
            for domain in domains {
                entries.push(WhitelistEntry {
                    plugin_id: plugin_id.clone(),
                    domain: domain.clone(),
                    approved: false,
                });
            }
        }

        entries
    }

    /// Get all approved entries for persistence
    pub fn get_approved(&self) -> Vec<WhitelistEntry> {
        let approved = self.approved.read();
        let mut entries = Vec::new();

        for (plugin_id, domains) in approved.iter() {
            for domain in domains {
                entries.push(WhitelistEntry {
                    plugin_id: plugin_id.clone(),
                    domain: domain.clone(),
                    approved: true,
                });
            }
        }

        entries
    }

    /// Get all entries (approved and pending)
    pub fn get_all_entries(&self) -> Vec<WhitelistEntry> {
        let mut entries = self.get_approved();
        entries.extend(self.get_pending());
        entries
    }
}

/// Normalize domain for comparison
fn normalize_domain(domain: &str) -> String {
    domain.to_lowercase().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_whitelist() {
        let wl = DomainWhitelist::new();
        assert_eq!(
            wl.check("test-plugin", "example.com"),
            AccessCheck::NotWhitelisted
        );
    }

    #[test]
    fn test_add_and_check() {
        let wl = DomainWhitelist::new();
        wl.add_pending("test-plugin", "dlsite.com");

        assert_eq!(
            wl.check("test-plugin", "dlsite.com"),
            AccessCheck::NeedsApproval
        );

        wl.approve("test-plugin", "dlsite.com");

        assert_eq!(wl.check("test-plugin", "dlsite.com"), AccessCheck::Allowed);
    }

    #[test]
    fn test_from_entries() {
        let entries = vec![
            WhitelistEntry::approved("plugin1", "dlsite.com"),
            WhitelistEntry::pending("plugin1", "getchu.com"),
        ];

        let wl = DomainWhitelist::from_entries(entries);

        assert_eq!(wl.check("plugin1", "dlsite.com"), AccessCheck::Allowed);
        assert_eq!(
            wl.check("plugin1", "getchu.com"),
            AccessCheck::NeedsApproval
        );
    }
}
