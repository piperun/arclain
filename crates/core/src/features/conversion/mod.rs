//! Archive conversion options and utilities

pub mod flatten;

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum ConvertFormat {
    Zip,
    SevenZ,
}

impl ConvertFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZ => "7z",
        }
    }

    pub fn sevenz_flag(&self) -> &'static str {
        match self {
            Self::Zip => "-tzip",
            Self::SevenZ => "-t7z",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompressionLevel {
    Fast,
    Normal,
    Max,
}

impl CompressionLevel {
    pub fn sevenz_flag(&self) -> &'static str {
        match self {
            Self::Fast => "-mx=1",
            Self::Normal => "-mx=5",
            Self::Max => "-mx=9",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub format: ConvertFormat,
    pub compression: CompressionLevel,
    pub password: Option<String>,
    pub flatten_nested: bool,
    pub strip_common_prefix: bool,
    pub output_path: Option<PathBuf>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            format: ConvertFormat::Zip,
            compression: CompressionLevel::Normal,
            password: None,
            flatten_nested: false,
            strip_common_prefix: false,
            output_path: None,
        }
    }
}

/// Find the longest common prefix across a list of names.
/// Returns empty string if no meaningful prefix (<5 chars) exists.
pub fn longest_common_prefix(names: &[&str]) -> String {
    if names.is_empty() || names.len() == 1 {
        return String::new();
    }

    let first = names[0];
    let mut prefix_len = first.len();

    for name in &names[1..] {
        let common = first
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix_len = prefix_len.min(common);
        if prefix_len == 0 {
            return String::new();
        }
    }

    let prefix: String = first.chars().take(prefix_len).collect();
    if prefix.len() < 5 {
        String::new()
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_extension() {
        assert_eq!(ConvertFormat::Zip.extension(), "zip");
        assert_eq!(ConvertFormat::SevenZ.extension(), "7z");
    }

    #[test]
    fn test_format_sevenz_flag() {
        assert_eq!(ConvertFormat::Zip.sevenz_flag(), "-tzip");
        assert_eq!(ConvertFormat::SevenZ.sevenz_flag(), "-t7z");
    }

    #[test]
    fn test_compression_flags() {
        assert_eq!(CompressionLevel::Fast.sevenz_flag(), "-mx=1");
        assert_eq!(CompressionLevel::Normal.sevenz_flag(), "-mx=5");
        assert_eq!(CompressionLevel::Max.sevenz_flag(), "-mx=9");
    }

    #[test]
    fn test_default_options() {
        let opts = ConvertOptions::default();
        assert_eq!(opts.format, ConvertFormat::Zip);
        assert_eq!(opts.compression, CompressionLevel::Normal);
        assert!(!opts.flatten_nested);
        assert!(!opts.strip_common_prefix);
    }

    #[test]
    fn test_common_prefix_silver_lining_example() {
        let names = [
            "AG - LK - Silver Linning Lingerie - Main.rar",
            "AG - LK - Silver Linning Lingerie - Patch Makeup.rar",
            "AG - LK - Silver Linning Lingerie - Patch No Clothes.rar",
        ];
        let prefix = longest_common_prefix(&names);
        assert_eq!(prefix, "AG - LK - Silver Linning Lingerie - ");
    }

    #[test]
    fn test_common_prefix_no_meaningful_prefix() {
        let names = ["Main.rar", "Variant.rar"];
        assert_eq!(longest_common_prefix(&names), "");
    }

    #[test]
    fn test_common_prefix_empty_list() {
        let names: [&str; 0] = [];
        assert_eq!(longest_common_prefix(&names), "");
    }

    #[test]
    fn test_common_prefix_single_item() {
        let names = ["only_one.rar"];
        assert_eq!(longest_common_prefix(&names), "");
    }

    #[test]
    fn test_common_prefix_short_is_rejected() {
        let names = ["My A.rar", "My B.rar"];
        assert_eq!(longest_common_prefix(&names), "");
    }
}
