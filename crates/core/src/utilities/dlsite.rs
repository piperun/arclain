//! DLsite utilities for detecting product codes
//!
//! This module provides simple utilities for detecting DLsite product codes
//! (RJ/VJ/BJ) in archive names.

use regex::Regex;

/// Detect DLsite product code from text
///
/// Returns the detected code (e.g., "RJ123456") if found, None otherwise.
/// Supports RJ (doujin), VJ (voice), and BJ (books) codes with 6-8 digits.
///
/// # Examples
/// ```
/// use arclain_core::utilities::dlsite::detect_dlsite_code;
///
/// assert_eq!(detect_dlsite_code("[RJ123456] Game Title"), Some("RJ123456".to_string()));
/// assert_eq!(detect_dlsite_code("Game.zip"), None);
/// ```
pub fn detect_dlsite_code(text: &str) -> Option<String> {
    // RJ/VJ/BJ followed by 6-8 digits, case insensitive
    let re = Regex::new(r"(?i)(RJ|VJ|BJ)(\d{6,8})").ok()?;

    if let Some(caps) = re.captures(text) {
        let prefix = caps.get(1)?.as_str().to_uppercase();
        let digits = caps.get(2)?.as_str();
        return Some(format!("{}{}", prefix, digits));
    }
    None
}

/// Check if a DLsite code is present in the text
pub fn has_dlsite_code(text: &str) -> bool {
    detect_dlsite_code(text).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_dlsite_code() {
        // Standard codes
        assert_eq!(
            detect_dlsite_code("RJ123456.zip"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("rj123456.rar"),
            Some("RJ123456".to_string())
        );

        // Inside brackets
        assert_eq!(
            detect_dlsite_code("[RJ123456] Game Title"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("(RJ123456) Game Title"),
            Some("RJ123456".to_string())
        );

        // 7-digit codes (newer)
        assert_eq!(
            detect_dlsite_code("RJ1234567.zip"),
            Some("RJ1234567".to_string())
        );

        // 8-digit codes
        assert_eq!(
            detect_dlsite_code("RJ12345678.zip"),
            Some("RJ12345678".to_string())
        );

        // VJ codes
        assert_eq!(
            detect_dlsite_code("VJ123456.zip"),
            Some("VJ123456".to_string())
        );

        // BJ codes
        assert_eq!(
            detect_dlsite_code("BJ123456.zip"),
            Some("BJ123456".to_string())
        );

        // No code
        assert_eq!(detect_dlsite_code("Game.zip"), None);
        assert_eq!(detect_dlsite_code("RJ12345.zip"), None); // Only 5 digits
    }

    #[test]
    fn test_has_dlsite_code() {
        assert!(has_dlsite_code("[RJ123456] Game Title"));
        assert!(!has_dlsite_code("Game.zip"));
    }
}
