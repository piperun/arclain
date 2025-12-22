//! DLSite code detection

use regex::Regex;

/// Detect DLSite product code from text (filename, folder name, etc.)
pub fn detect_dlsite_code(text: &str) -> Option<String> {
    // Pattern: RJ, VJ, or BJ followed by 6-8 digits
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
    fn test_detect_codes() {
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
}
