//! DLSite Metadata Plugin
//!
//! This plugin extracts DLSite product codes from archive filenames and
//! fetches metadata from the DLSite API.

#![no_std]

extern crate alloc;

#[macro_use]
extern crate archust_plugin_sdk;

use alloc::format;
use alloc::string::{String, ToString};
use archust_plugin_sdk::prelude::*;
use serde_json::json;

plugin_metadata!(
    "dlsite-metadata",
    "DLSite Metadata Extractor",
    "1.0.0",
    "Archust Team",
    "Extracts DLSite product codes and fetches metadata"
);

plugin_init!();
plugin_cleanup!();

/// DLSite product code
#[derive(Debug, Clone)]
pub struct DLSiteCode {
    pub prefix: String, // RJ, VJ, or BJ
    pub number: String, // 6-8 digits
}

impl DLSiteCode {
    pub fn full_code(&self) -> String {
        format!("{}{}", self.prefix, self.number)
    }
}

/// Extract DLSite code from filename
/// Supports patterns like:
/// - RJ123456
/// - [site] RJ123456 Game Name.zip
/// - VJ01234567
/// - BJ12345678
pub fn extract_dlsite_code(filename: &str) -> Option<DLSiteCode> {
    // Pattern: (RJ|VJ|BJ) followed by 6-8 digits
    let filename_upper = filename.to_uppercase();

    // Find RJ, VJ, or BJ prefix
    let prefixes = ["RJ", "VJ", "BJ"];

    for prefix in &prefixes {
        if let Some(pos) = filename_upper.find(prefix) {
            let after_prefix = &filename_upper[pos + prefix.len()..];

            // Extract digits
            let mut digits = String::new();
            for ch in after_prefix.chars() {
                if ch.is_ascii_digit() {
                    digits.push(ch);
                } else {
                    break;
                }
            }

            // Validate length (6-8 digits)
            if digits.len() >= 6 && digits.len() <= 8 {
                return Some(DLSiteCode {
                    prefix: prefix.to_string(),
                    number: digits,
                });
            }
        }
    }

    None
}

/// Clean filename from download site tags
pub fn clean_filename(filename: &str) -> String {
    let mut cleaned = filename.to_string();

    // Remove common download site tags
    let tags = [
        "[DLsite]",
        "[dlsite]",
        "[Download]",
        "[download]",
        "(DLsite)",
        "(dlsite)",
    ];

    for tag in &tags {
        cleaned = cleaned.replace(tag, "");
    }

    // Remove extra spaces
    while cleaned.contains("  ") {
        cleaned = cleaned.replace("  ", " ");
    }

    cleaned.trim().to_string()
}

/// Extract title from filename after DLSite code
pub fn extract_title_from_filename(filename: &str, code: &DLSiteCode) -> Option<String> {
    let full_code = code.full_code();
    let filename_upper = filename.to_uppercase();

    if let Some(pos) = filename_upper.find(&full_code) {
        let after_code = &filename[pos + full_code.len()..];

        // Remove file extension
        let without_ext = if let Some(ext_pos) = after_code.rfind('.') {
            &after_code[..ext_pos]
        } else {
            after_code
        };

        // Clean up
        let cleaned = clean_filename(without_ext);

        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }

    None
}

/// Fetch metadata from DLSite API
///
/// Note: This is a placeholder implementation. The actual DLSite API
/// requires authentication and has specific endpoint formats.
pub fn fetch_dlsite_metadata(code: &DLSiteCode) -> Result<serde_json::Value, i32> {
    log(
        LogLevel::Info,
        &format!("Fetching metadata for {}", code.full_code()),
    );

    // Construct API URL (this is a placeholder - real API would be different)
    let url = format!(
        "https://www.dlsite.com/maniax/work/=/product_id/{}.html",
        code.full_code()
    );

    // Make HTTP request
    match http_get(&url) {
        Ok(response) => {
            log(LogLevel::Info, "Successfully fetched metadata");

            // Parse HTML response (in production, would use proper HTML parser)
            // For now, return a placeholder JSON
            Ok(json!({
                "code": code.full_code(),
                "title": "Title would be extracted from HTML",
                "circle": "Circle name would be extracted",
                "release_date": "Release date would be extracted",
                "url": url
            }))
        }
        Err(error_code) => {
            log(
                LogLevel::Error,
                &format!("HTTP request failed: {}", error_code),
            );
            Err(error_code)
        }
    }
}

/// Plugin event handler
#[no_mangle]
pub extern "C" fn plugin_on_event(event_ptr: *const u8, event_len: usize) -> i32 {
    // Read event JSON from memory
    let event_bytes = unsafe { core::slice::from_raw_parts(event_ptr, event_len) };

    let event_str = match core::str::from_utf8(event_bytes) {
        Ok(s) => s,
        Err(_) => {
            log(LogLevel::Error, "Invalid UTF-8 in event");
            return -1;
        }
    };

    // Parse event
    let event: PluginEvent = match serde_json::from_str(event_str) {
        Ok(e) => e,
        Err(_) => {
            log(LogLevel::Error, "Failed to parse event JSON");
            return -1;
        }
    };

    // Handle OnArchiveOpen event
    match event {
        PluginEvent::OnArchiveOpen { ref path, .. } => {
            log(LogLevel::Info, &format!("Archive opened: {}", path));

            // Extract DLSite code from filename
            if let Some(code) = extract_dlsite_code(path) {
                log(
                    LogLevel::Info,
                    &format!("Found DLSite code: {}", code.full_code()),
                );

                // Extract title
                if let Some(title) = extract_title_from_filename(path, &code) {
                    log(LogLevel::Info, &format!("Extracted title: {}", title));
                }

                // Fetch metadata (commented out for now to avoid real API calls)
                // match fetch_dlsite_metadata(&code) {
                //     Ok(metadata) => {
                //         log(LogLevel::Info, "Metadata fetched successfully");
                //         // Would return metadata response here
                //     }
                //     Err(_) => {
                //         log(LogLevel::Warn, "Failed to fetch metadata");
                //     }
                // }

                // Return metadata response (placeholder)
                let response = PluginResponse::Metadata {
                    data: json!({
                        "dlsite_code": code.full_code(),
                        "extracted_title": extract_title_from_filename(path, &code),
                    }),
                };

                // For now, just return success
                // Full response serialization will be implemented in Phase 3
                return 0;
            }

            0
        }
        _ => 0, // Ignore other events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_dlsite_code_rj() {
        let code = extract_dlsite_code("RJ123456 Game.zip").unwrap();
        assert_eq!(code.prefix, "RJ");
        assert_eq!(code.number, "123456");
    }

    #[test]
    fn test_extract_dlsite_code_vj() {
        let code = extract_dlsite_code("[DLsite] VJ01234567 Visual Novel.zip").unwrap();
        assert_eq!(code.prefix, "VJ");
        assert_eq!(code.number, "01234567");
    }

    #[test]
    fn test_extract_dlsite_code_bj() {
        let code = extract_dlsite_code("BJ12345678.zip").unwrap();
        assert_eq!(code.prefix, "BJ");
        assert_eq!(code.number, "12345678");
    }

    #[test]
    fn test_extract_dlsite_code_with_tags() {
        let code = extract_dlsite_code("[Download] RJ123456 Game Name.zip").unwrap();
        assert_eq!(code.full_code(), "RJ123456");
    }

    #[test]
    fn test_clean_filename() {
        let cleaned = clean_filename("[DLsite] [Download] Game Name.zip");
        assert_eq!(cleaned, "Game Name.zip");
    }

    #[test]
    fn test_extract_title() {
        let code = DLSiteCode {
            prefix: "RJ".to_string(),
            number: "123456".to_string(),
        };

        let title = extract_title_from_filename("RJ123456 Cool Game.zip", &code).unwrap();
        assert_eq!(title, "Cool Game");
    }

    #[test]
    fn test_extract_title_with_tags() {
        let code = DLSiteCode {
            prefix: "RJ".to_string(),
            number: "123456".to_string(),
        };

        let title =
            extract_title_from_filename("[DLsite] RJ123456 [Download] Cool Game.zip", &code)
                .unwrap();
        assert_eq!(title, "Cool Game");
    }

    #[test]
    fn test_no_code_found() {
        let result = extract_dlsite_code("regular_game.zip");
        assert!(result.is_none());
    }
}
