//! Domain parsing utilities
//!
//! Extracts the effective domain from URLs, handling subdomain tricks.

use super::types::{DomainInfo, DomainWarning};
use url::Url;

/// Parse a URL and extract domain information
pub fn parse_url(url_str: &str) -> Result<DomainInfo, String> {
    let url = Url::parse(url_str).map_err(|e| format!("Invalid URL: {}", e))?;

    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_lowercase();

    let mut warnings = Vec::new();

    // Check for IP address
    if is_ip_address(&host) {
        warnings.push(DomainWarning::IpAddress { ip: host.clone() });
        return Ok(DomainInfo {
            full_url: url_str.to_string(),
            effective_domain: host.clone(),
            host: host.clone(),
            tld: String::new(),
            warnings,
        });
    }

    // Check for localhost/private
    if is_localhost_or_private(&host) {
        warnings.push(DomainWarning::LocalhostOrPrivate);
    }

    // Split domain into parts
    let parts: Vec<&str> = host.split('.').collect();

    // Extract TLD and effective domain
    let (tld, effective_domain) = extract_effective_domain(&parts);

    // Check for excessive subdomains
    let subdomain_count = parts
        .len()
        .saturating_sub(effective_domain.split('.').count());
    if subdomain_count > 3 {
        warnings.push(DomainWarning::ExcessiveSubdomains {
            count: subdomain_count,
        });
    }

    // Check for suspicious subdomain patterns
    if let Some(warning) = check_suspicious_subdomain(&parts, &effective_domain) {
        warnings.push(warning);
    }

    Ok(DomainInfo {
        full_url: url_str.to_string(),
        effective_domain,
        host,
        tld,
        warnings,
    })
}

/// Check if host is an IP address
fn is_ip_address(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

/// Check if host is localhost or private network
fn is_localhost_or_private(host: &str) -> bool {
    host == "localhost"
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("172.16.")
        || host.starts_with("172.17.")
        || host.starts_with("172.18.")
        || host.starts_with("172.19.")
        || host.starts_with("172.2")
        || host.starts_with("172.30.")
        || host.starts_with("172.31.")
}

/// Known multi-part TLDs (e.g., .co.jp, .com.au)
const MULTI_PART_TLDS: &[&str] = &[
    "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp", "com.au", "net.au", "org.au", "co.uk", "org.uk",
    "co.nz", "com.br", "co.kr", "or.kr",
];

/// Extract the effective/registrable domain
fn extract_effective_domain(parts: &[&str]) -> (String, String) {
    if parts.is_empty() {
        return (String::new(), String::new());
    }

    // Check for multi-part TLDs
    if parts.len() >= 3 {
        let potential_tld = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        if MULTI_PART_TLDS.contains(&potential_tld.as_str()) {
            let effective = format!(
                "{}.{}.{}",
                parts[parts.len() - 3],
                parts[parts.len() - 2],
                parts[parts.len() - 1]
            );
            return (potential_tld, effective);
        }
    }

    // Standard single TLD
    if parts.len() >= 2 {
        let tld = parts[parts.len() - 1].to_string();
        let effective = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        (tld, effective)
    } else {
        (parts[0].to_string(), parts[0].to_string())
    }
}

/// Check for subdomains that look like legitimate domains
fn check_suspicious_subdomain(parts: &[&str], effective_domain: &str) -> Option<DomainWarning> {
    // Known legitimate domains that scammers impersonate
    const IMPERSONATION_TARGETS: &[&str] = &[
        "google",
        "microsoft",
        "apple",
        "amazon",
        "paypal",
        "facebook",
        "twitter",
        "instagram",
        "netflix",
        "bank",
        "secure",
        "login",
        "verify",
        "account",
        "update",
    ];

    // Check subdomains (all parts except the effective domain parts)
    let effective_parts: Vec<&str> = effective_domain.split('.').collect();
    let subdomain_parts = &parts[..parts.len().saturating_sub(effective_parts.len())];

    for subdomain in subdomain_parts {
        let subdomain_lower = subdomain.to_lowercase();
        for target in IMPERSONATION_TARGETS {
            if subdomain_lower.contains(target) {
                return Some(DomainWarning::SuspiciousSubdomain {
                    subdomain: subdomain.to_string(),
                    looks_like: target.to_string(),
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_domain() {
        let info = parse_url("https://dlsite.com/page").unwrap();
        assert_eq!(info.effective_domain, "dlsite.com");
        assert_eq!(info.tld, "com");
        assert!(info.warnings.is_empty());
    }

    #[test]
    fn test_subdomain() {
        let info = parse_url("https://api.dlsite.com/v1/products").unwrap();
        assert_eq!(info.effective_domain, "dlsite.com");
        assert_eq!(info.host, "api.dlsite.com");
    }

    #[test]
    fn test_multi_part_tld() {
        let info = parse_url("https://example.co.jp/page").unwrap();
        assert_eq!(info.effective_domain, "example.co.jp");
        assert_eq!(info.tld, "co.jp");
    }

    #[test]
    fn test_suspicious_subdomain() {
        let info = parse_url("https://google.com.evil.ru/login").unwrap();
        assert_eq!(info.effective_domain, "evil.ru");
        assert!(info.has_warnings());
    }

    #[test]
    fn test_ip_address() {
        let info = parse_url("http://192.168.1.1/admin").unwrap();
        assert!(info
            .warnings
            .iter()
            .any(|w| matches!(w, DomainWarning::IpAddress { .. })));
    }
}
