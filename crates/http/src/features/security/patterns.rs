//! Suspicious URL pattern detection

use super::types::DomainWarning;

/// Check URL for suspicious patterns
pub fn check_patterns(url: &str, domain: &str) -> Vec<DomainWarning> {
    let mut warnings = Vec::new();

    // Check for suspicious keywords in domain
    let suspicious_keywords = check_suspicious_keywords(domain);
    if !suspicious_keywords.is_empty() {
        warnings.push(DomainWarning::SuspiciousKeywords {
            keywords: suspicious_keywords,
        });
    }

    // Check for URL encoding tricks
    if has_suspicious_encoding(url) {
        warnings.push(DomainWarning::SuspiciousEncoding);
    }

    // Check for unusual TLDs often used in phishing
    if let Some(warning) = check_unusual_tld(domain) {
        warnings.push(warning);
    }

    warnings
}

/// Detect suspicious keywords in domain
fn check_suspicious_keywords(domain: &str) -> Vec<String> {
    const SUSPICIOUS: &[&str] = &[
        "secure-", "-secure", "login-", "-login", "signin-", "-signin", "verify-", "-verify",
        "update-", "-update", "confirm-", "-confirm", "account-", "-account", "banking", "wallet",
        "crypto",
    ];

    let domain_lower = domain.to_lowercase();
    SUSPICIOUS
        .iter()
        .filter(|kw| domain_lower.contains(*kw))
        .map(|s| s.to_string())
        .collect()
}

/// Check for URL encoding that might hide the real destination
fn has_suspicious_encoding(url: &str) -> bool {
    // Check for encoded slashes, at signs, or colons in suspicious places
    let suspicious_patterns = [
        "%2F%2F", // Encoded //
        "%40",    // Encoded @
        "%3A%2F", // Encoded :/
        "%00",    // Null byte
    ];

    suspicious_patterns.iter().any(|p| url.contains(p))
}

/// TLDs commonly associated with phishing/spam
const SUSPICIOUS_TLDS: &[&str] = &[
    "tk", "ml", "ga", "cf", "gq", // Free TLDs often abused
    "xyz", "top", "wang", "win", "bid", "click", "link", "download", "stream", "racing",
];

/// Check for unusual/suspicious TLDs
fn check_unusual_tld(domain: &str) -> Option<DomainWarning> {
    let parts: Vec<&str> = domain.split('.').collect();
    if let Some(tld) = parts.last() {
        let tld_lower = tld.to_lowercase();
        if SUSPICIOUS_TLDS.contains(&tld_lower.as_str()) {
            return Some(DomainWarning::UnusualTld {
                tld: tld.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suspicious_keywords() {
        let keywords = check_suspicious_keywords("secure-login.example.com");
        assert!(keywords.contains(&"secure-".to_string()));
        assert!(keywords.contains(&"-login".to_string()));
    }

    #[test]
    fn test_clean_domain() {
        let keywords = check_suspicious_keywords("dlsite.com");
        assert!(keywords.is_empty());
    }

    #[test]
    fn test_suspicious_tld() {
        let warning = check_unusual_tld("example.tk");
        assert!(warning.is_some());
    }
}
