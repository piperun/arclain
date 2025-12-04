use serde::Deserialize;
use std::collections::HashMap;

/// Configuration for title filtering
#[derive(Debug, Clone, Deserialize)]
pub struct TitleFilterConfig {
    #[serde(default)]
    pub filters: FilterSettings,
    #[serde(default)]
    pub replacements: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilterSettings {
    #[serde(default = "default_invalid_chars")]
    pub invalid_chars: String,
    #[serde(default = "default_replacement")]
    pub replacement: String,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    #[serde(default = "default_true")]
    pub trim_whitespace: bool,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            invalid_chars: default_invalid_chars(),
            replacement: default_replacement(),
            max_length: default_max_length(),
            trim_whitespace: true,
        }
    }
}

fn default_invalid_chars() -> String {
    r#"/:*?"<>|\"#.to_string()
}

fn default_replacement() -> String {
    "_".to_string()
}

fn default_max_length() -> usize {
    255
}

fn default_true() -> bool {
    true
}

/// Load title filter configuration from TOML file
pub fn load_config() -> Option<TitleFilterConfig> {
    // Try to load from assets directory
    let config_path = std::path::PathBuf::from("assets/title_filters.toml");

    if !config_path.exists() {
        return None;
    }

    match std::fs::read_to_string(&config_path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(e) => {
                tracing::warn!("Failed to parse title_filters.toml: {}. Using fallback.", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to read title_filters.toml: {}. Using fallback.", e);
            None
        }
    }
}

/// Sanitize a title for use in folder names
///
/// This function replaces invalid filesystem characters with safe alternatives.
/// It uses TOML configuration if available, otherwise falls back to hardcoded rules.
pub fn sanitize_title(title: &str) -> String {
    let config = load_config();
    sanitize_title_with_config(title, config.as_ref())
}

/// Sanitize a title with an explicit configuration
pub fn sanitize_title_with_config(title: &str, config: Option<&TitleFilterConfig>) -> String {
    let mut result = title.to_string();

    if let Some(cfg) = config {
        // Apply custom character replacements first
        for (from, to) in &cfg.replacements {
            result = result.replace(from, to);
        }

        // Replace invalid characters
        let invalid_chars: Vec<char> = cfg.filters.invalid_chars.chars().collect();
        result = result
            .chars()
            .map(|c| {
                if invalid_chars.contains(&c) || c.is_control() {
                    cfg.filters.replacement.chars().next().unwrap_or('_')
                } else {
                    c
                }
            })
            .collect();

        // Trim whitespace if configured
        if cfg.filters.trim_whitespace {
            result = result.trim().to_string();
        }

        // Truncate if needed
        if result.len() > cfg.filters.max_length {
            result.truncate(cfg.filters.max_length);
            // Ensure we don't cut in the middle of a multi-byte character
            while !result.is_char_boundary(result.len()) && result.len() > 0 {
                result.pop();
            }
        }
    } else {
        // Fallback: hardcoded sanitization
        result = result
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            })
            .collect();

        result = result.trim().to_string();

        // Truncate to 255 chars (typical filesystem limit)
        if result.len() > 255 {
            result.truncate(255);
            while !result.is_char_boundary(result.len()) && result.len() > 0 {
                result.pop();
            }
        }
    }

    // Ensure we don't return an empty string
    if result.is_empty() {
        result = "untitled".to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_basic() {
        assert_eq!(sanitize_title("Normal Title"), "Normal Title");
    }

    #[test]
    fn test_sanitize_invalid_chars() {
        assert_eq!(
            sanitize_title("Test/Game:Full*Version?"),
            "Test_Game_Full_Version_"
        );
    }

    #[test]
    fn test_sanitize_japanese() {
        // Japanese characters should be preserved
        assert_eq!(sanitize_title("テストゲーム"), "テストゲーム");
    }

    #[test]
    fn test_sanitize_mixed() {
        assert_eq!(
            sanitize_title("[RJ123456] テスト/ゲーム:完全版"),
            "[RJ123456] テスト_ゲーム_完全版"
        );
    }

    #[test]
    fn test_sanitize_control_chars() {
        let title_with_control = format!("Test{}Game", '\x00');
        assert_eq!(sanitize_title(&title_with_control), "Test_Game");
    }

    #[test]
    fn test_sanitize_empty() {
        assert_eq!(sanitize_title(""), "untitled");
        assert_eq!(sanitize_title("   "), "untitled");
    }

    #[test]
    fn test_sanitize_with_config() {
        let mut replacements = HashMap::new();
        replacements.insert("【".to_string(), "[".to_string());
        replacements.insert("】".to_string(), "]".to_string());

        let config = TitleFilterConfig {
            filters: FilterSettings::default(),
            replacements,
        };

        assert_eq!(
            sanitize_title_with_config("【Test】Game", Some(&config)),
            "[Test]Game"
        );
    }

    #[test]
    fn test_sanitize_long_title() {
        let long_title = "A".repeat(300);
        let sanitized = sanitize_title(&long_title);
        assert!(sanitized.len() <= 255);
        assert_eq!(sanitized, "A".repeat(255));
    }
}
