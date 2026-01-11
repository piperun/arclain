//! Source detection from text

use crate::MetadataSource;
use regex::Regex;

/// Detect metadata source and ID from text (filename, folder name, URL, etc.)
///
/// Returns (Source, ExternalID) if detected.
///
/// # Examples
/// ```
/// use gameta_lib::detect::detect_source;
/// use gameta_lib::MetadataSource;
///
/// let result = detect_source("My Game RJ123456.zip");
/// assert_eq!(result, Some((MetadataSource::DLSite, "RJ123456".to_string())));
/// ```
pub fn detect_source(text: &str) -> Option<(MetadataSource, String)> {
    // Try DLSite first (most common in this project)
    if let Some(id) = detect_dlsite_code(text) {
        return Some((MetadataSource::DLSite, id));
    }

    // Future: Steam, itch.io, etc.

    None
}

/// Detect DLSite product code from text
///
/// Pattern: RJ, VJ, or BJ followed by 6-8 digits
pub fn detect_dlsite_code(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)(RJ|VJ|BJ)(\d{6,8})").ok()?;

    re.captures(text).map(|caps| {
        let prefix = caps.get(1).unwrap().as_str().to_uppercase();
        let number = caps.get(2).unwrap().as_str();
        format!("{}{}", prefix, number)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_dlsite_code() {
        assert_eq!(detect_dlsite_code("RJ123456"), Some("RJ123456".to_string()));
        assert_eq!(
            detect_dlsite_code("vj12345678"),
            Some("VJ12345678".to_string())
        );
        assert_eq!(
            detect_dlsite_code("[Circle] Game Title RJ999999"),
            Some("RJ999999".to_string())
        );
        assert_eq!(detect_dlsite_code("no code here"), None);
    }

    #[test]
    fn test_detect_source() {
        let (source, id) = detect_source("RJ123456.zip").unwrap();
        assert_eq!(source, MetadataSource::DLSite);
        assert_eq!(id, "RJ123456");
    }
}
