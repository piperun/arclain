//! Domain whitelist management
//!
//! Controls which domains each plugin can access.

use super::types::{AccessCheck, WhitelistEntry};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Manages domain access permissions for plugins
#[derive(Debug, Default)]
pub struct DomainWhitelist {
    /// plugin_id -> domains independently approved by the user or persistence.
    approved: RwLock<HashMap<String, HashSet<String>>>,
    /// plugin_id -> domains granted by the currently loaded manifest.
    ///
    /// Keeping this ownership separate lets reload/unload revoke only the
    /// manifest grant without erasing an overlapping user approval.
    manifest_approved: RwLock<HashMap<String, HashSet<String>>>,
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
            manifest_approved: RwLock::new(HashMap::new()),
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

        let manifest_approved = self.manifest_approved.read();
        if let Some(domains) = manifest_approved.get(plugin_id) {
            if domains.contains(&domain_normalized) {
                return AccessCheck::Allowed;
            }
        }
        drop(manifest_approved);

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

    /// Replace the domains granted by a plugin's current manifest.
    ///
    /// User approvals live in a separate ownership layer and therefore
    /// survive manifest replacement even when both layers grant the same
    /// domain.
    pub fn replace_manifest_domains<I, S>(&self, plugin_id: &str, domains: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let domains: HashSet<String> = domains
            .into_iter()
            .map(|domain| normalize_domain(domain.as_ref()))
            .collect();

        if let Some(pending) = self.pending.write().get_mut(plugin_id) {
            pending.retain(|domain| !domains.contains(domain));
        }

        let mut manifest_approved = self.manifest_approved.write();
        if domains.is_empty() {
            manifest_approved.remove(plugin_id);
        } else {
            manifest_approved.insert(plugin_id.to_string(), domains);
        }
    }

    /// Revoke every domain owned only by the unloaded plugin manifest.
    pub fn clear_manifest_domains(&self, plugin_id: &str) {
        self.manifest_approved.write().remove(plugin_id);
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
        let mut seen: HashSet<(String, String)> = entries
            .iter()
            .map(|entry| (entry.plugin_id.clone(), entry.domain.clone()))
            .collect();
        for (plugin_id, domains) in self.manifest_approved.read().iter() {
            for domain in domains {
                if seen.insert((plugin_id.clone(), domain.clone())) {
                    entries.push(WhitelistEntry {
                        plugin_id: plugin_id.clone(),
                        domain: domain.clone(),
                        approved: true,
                    });
                }
            }
        }
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

    #[test]
    fn replacing_manifest_domains_revokes_only_that_ownership() {
        let wl = DomainWhitelist::new();
        wl.approve("plugin-a", "user-approved.test");
        wl.approve("plugin-a", "shared-ownership.test");
        wl.replace_manifest_domains("plugin-a", ["old-manifest.test", "shared-ownership.test"]);
        wl.replace_manifest_domains("plugin-b", ["old-manifest.test"]);

        wl.replace_manifest_domains("plugin-a", ["new-manifest.test"]);

        assert_eq!(
            wl.check("plugin-a", "old-manifest.test"),
            AccessCheck::NotWhitelisted,
            "a replacement manifest retained a removed domain",
        );
        assert_eq!(
            wl.check("plugin-a", "new-manifest.test"),
            AccessCheck::Allowed,
        );
        assert_eq!(
            wl.check("plugin-a", "user-approved.test"),
            AccessCheck::Allowed,
            "manifest replacement revoked an independent user approval",
        );
        assert_eq!(
            wl.check("plugin-a", "shared-ownership.test"),
            AccessCheck::Allowed,
            "removing manifest ownership revoked overlapping user ownership",
        );
        assert_eq!(
            wl.check("plugin-b", "old-manifest.test"),
            AccessCheck::Allowed,
            "replacing one plugin's manifest domains affected another plugin",
        );
        assert!(
            !wl.get_approved()
                .iter()
                .any(|entry| entry.domain == "new-manifest.test"),
            "manifest ownership leaked into persistence entries",
        );
        assert!(
            wl.get_all_entries()
                .iter()
                .any(|entry| entry.plugin_id == "plugin-a"
                    && entry.domain == "new-manifest.test"
                    && entry.approved),
            "effective manifest approval disappeared from UI observability",
        );

        wl.clear_manifest_domains("plugin-a");

        assert_eq!(
            wl.check("plugin-a", "new-manifest.test"),
            AccessCheck::NotWhitelisted,
            "unload retained manifest-owned access",
        );
        assert_eq!(
            wl.check("plugin-a", "user-approved.test"),
            AccessCheck::Allowed,
        );
        assert_eq!(
            wl.check("plugin-a", "shared-ownership.test"),
            AccessCheck::Allowed,
        );
        assert_eq!(
            wl.check("plugin-b", "old-manifest.test"),
            AccessCheck::Allowed,
        );
    }
}
