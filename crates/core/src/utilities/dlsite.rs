//! DLsite utilities for detecting product codes
//!
//! Delegates to gameta_lib for the actual detection logic.

/// Detect DLsite product code from text
///
/// Returns the detected code (e.g., "RJ123456") if found, None otherwise.
/// Supports RJ (doujin), VJ (voice), and BJ (books) codes with 6-8 digits.
pub fn detect_dlsite_code(text: &str) -> Option<String> {
    gameta_lib::detect::detect_dlsite_code(text)
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
        assert_eq!(
            detect_dlsite_code("RJ123456.zip"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("rj123456.rar"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("[RJ123456] Game Title"),
            Some("RJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("VJ123456.zip"),
            Some("VJ123456".to_string())
        );
        assert_eq!(
            detect_dlsite_code("BJ123456.zip"),
            Some("BJ123456".to_string())
        );
        assert_eq!(detect_dlsite_code("Game.zip"), None);
        assert_eq!(detect_dlsite_code("RJ12345.zip"), None);
    }

    #[test]
    fn test_has_dlsite_code() {
        assert!(has_dlsite_code("[RJ123456] Game Title"));
        assert!(!has_dlsite_code("Game.zip"));
    }
}
