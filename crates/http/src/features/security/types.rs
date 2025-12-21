//! Types for domain security analysis

use serde::{Deserialize, Serialize};

/// Information about a domain extracted from a URL
#[derive(Debug, Clone)]
pub struct DomainInfo {
    /// The original full URL
    pub full_url: String,
    /// The effective/registrable domain (e.g., "dlsite.com" not "api.dlsite.com")
    pub effective_domain: String,
    /// The host (may include subdomain)
    pub host: String,
    /// Top-level domain (e.g., "com", "co.jp")
    pub tld: String,
    /// Any security warnings detected
    pub warnings: Vec<DomainWarning>,
}

impl DomainInfo {
    /// Check if the domain has any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Check if the domain has any critical warnings
    pub fn has_critical_warnings(&self) -> bool {
        self.warnings.iter().any(|w| w.is_critical())
    }
}

/// Security warnings about a domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DomainWarning {
    /// Detected homograph attack (lookalike characters)
    HomographDetected {
        suspicious_char: char,
        position: usize,
        looks_like: char,
    },

    /// Subdomain designed to look like another domain
    SuspiciousSubdomain {
        subdomain: String,
        looks_like: String,
    },

    /// Unusual or suspicious TLD
    UnusualTld { tld: String },

    /// URL uses IP address instead of domain
    IpAddress { ip: String },

    /// URL points to localhost or private network
    LocalhostOrPrivate,

    /// URL contains encoded characters that might hide the real destination
    SuspiciousEncoding,

    /// Domain has excessive subdomains
    ExcessiveSubdomains { count: usize },

    /// Domain contains suspicious keywords
    SuspiciousKeywords { keywords: Vec<String> },
}

impl DomainWarning {
    /// Check if this is a critical warning that should block the request
    pub fn is_critical(&self) -> bool {
        matches!(
            self,
            DomainWarning::HomographDetected { .. }
                | DomainWarning::LocalhostOrPrivate
                | DomainWarning::IpAddress { .. }
        )
    }

    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            DomainWarning::HomographDetected {
                suspicious_char,
                looks_like,
                ..
            } => {
                format!(
                    "Character '{}' looks like '{}' but is from a different alphabet",
                    suspicious_char, looks_like
                )
            }
            DomainWarning::SuspiciousSubdomain {
                subdomain,
                looks_like,
            } => {
                format!(
                    "Subdomain '{}' is designed to look like '{}'",
                    subdomain, looks_like
                )
            }
            DomainWarning::UnusualTld { tld } => {
                format!("Unusual top-level domain: .{}", tld)
            }
            DomainWarning::IpAddress { ip } => {
                format!("URL uses IP address ({}) instead of domain name", ip)
            }
            DomainWarning::LocalhostOrPrivate => {
                "URL points to localhost or private network".to_string()
            }
            DomainWarning::SuspiciousEncoding => {
                "URL contains encoded characters that might hide the destination".to_string()
            }
            DomainWarning::ExcessiveSubdomains { count } => {
                format!("URL has {} subdomain levels (unusual)", count)
            }
            DomainWarning::SuspiciousKeywords { keywords } => {
                format!(
                    "Domain contains suspicious keywords: {}",
                    keywords.join(", ")
                )
            }
        }
    }
}
