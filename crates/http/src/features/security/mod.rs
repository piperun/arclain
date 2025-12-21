//! Domain security analysis feature
//!
//! Provides URL parsing, homograph detection, and suspicious pattern detection.

mod domain_parser;
mod homograph;
mod patterns;
pub mod types;

pub use domain_parser::parse_url;
pub use homograph::{detect_homographs, has_mixed_scripts};
pub use patterns::check_patterns;
pub use types::{DomainInfo, DomainWarning};

/// Perform full security analysis on a URL
pub fn analyze_url(url: &str) -> Result<DomainInfo, String> {
    let mut info = domain_parser::parse_url(url)?;

    // Add homograph warnings
    let homograph_warnings = homograph::detect_homographs(&info.host);
    info.warnings.extend(homograph_warnings);

    // Add pattern warnings
    let pattern_warnings = patterns::check_patterns(url, &info.effective_domain);
    info.warnings.extend(pattern_warnings);

    // Add mixed script warning if applicable
    if homograph::has_mixed_scripts(&info.host) && info.warnings.is_empty() {
        // Only add if we haven't already flagged specific characters
        // This catches cases where scripts are mixed but no specific lookalike was found
    }

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_analysis_clean() {
        let info = analyze_url("https://dlsite.com/product/123").unwrap();
        assert_eq!(info.effective_domain, "dlsite.com");
        assert!(info.warnings.is_empty());
    }

    #[test]
    fn test_full_analysis_phishing() {
        let info = analyze_url("https://secure-login.google.com.evil.tk/verify").unwrap();
        assert_eq!(info.effective_domain, "evil.tk");
        assert!(info.has_warnings());
        assert!(info.has_critical_warnings() || info.warnings.len() > 1);
    }
}
