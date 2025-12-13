//! Password matching helper for auto-detecting passwords from rules.
//!
//! This module provides password matching logic that was previously in ConfigStore.
//! It takes a list of PassRule and matches against archive names/file entries.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A password matching rule
#[derive(Clone, Serialize, Deserialize)]
pub struct PassRule {
    pub name: String,
    pub pattern: String,
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

// Custom Debug implementation to avoid logging passwords
impl std::fmt::Debug for PassRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassRule")
            .field("name", &self.name)
            .field("pattern", &self.pattern)
            .field("password", &"[REDACTED]")
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl PassRule {
    pub fn to_regex(&self) -> Option<Regex> {
        Regex::new(&self.pattern).ok()
    }
}

/// Find the first matching password from a list of rules.
///
/// Matches against both the archive filename and file entries inside.
/// Rules are sorted by priority (descending) before matching.
pub fn auto_password_for(
    rules: &[PassRule],
    archive_path: Option<&str>,
    filenames: &[String],
) -> Option<String> {
    let mut sorted_rules: Vec<&PassRule> = rules.iter().filter(|r| r.enabled).collect();
    sorted_rules.sort_by_key(|r| std::cmp::Reverse(r.priority));

    for rule in sorted_rules {
        if let Some(re) = rule.to_regex() {
            // First check archive filename if provided
            if let Some(archive) = archive_path {
                // Extract just the filename from the full path
                let archive_name = Path::new(archive)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(archive);

                if re.is_match(archive_name) {
                    return Some(rule.password.clone());
                }
            }

            // Also check internal file paths for backwards compatibility
            if filenames.iter().any(|f| re.is_match(f)) {
                return Some(rule.password.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_matching_by_archive_name() {
        let rules = vec![PassRule {
            name: "Test".to_string(),
            pattern: r"test\.zip".to_string(),
            password: "secret".to_string(),
            priority: 10,
            enabled: true,
        }];

        let result = auto_password_for(&rules, Some("C:\\foo\\test.zip"), &[]);
        assert_eq!(result, Some("secret".to_string()));
    }

    #[test]
    fn test_password_matching_disabled_rule() {
        let rules = vec![PassRule {
            name: "Disabled".to_string(),
            pattern: r"test\.zip".to_string(),
            password: "secret".to_string(),
            priority: 10,
            enabled: false,
        }];

        let result = auto_password_for(&rules, Some("test.zip"), &[]);
        assert_eq!(result, None);
    }
}
