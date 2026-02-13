//! File entry data models used across features

/// Represents a single file or folder entry in the file list
#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub name: String, // Display name (basename only)
    pub path: String, // Full path within archive (for operations)
    pub size: String,
    pub compressed: String,
    pub ratio: String,
    pub modified: String,
    pub crc32: String,
    pub encrypted: bool,
    pub is_folder: bool,
    pub selected: bool,
}

/// Column identifiers for sorting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Type,
    Size,
    Compressed,
    Ratio,
    Modified,
    Crc32,
    Encrypted,
}

/// Current sort state (column + direction)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortState {
    pub column: SortColumn,
    pub ascending: bool,
}

impl Default for SortState {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            ascending: true,
        }
    }
}

/// Parse a human-readable size string (e.g., "12.3 KB") to bytes
pub fn parse_size_to_bytes(s: &str) -> u64 {
    let mut parts = s.split_whitespace();
    let num_str = parts.next().unwrap_or("0");
    let unit = parts.next().unwrap_or("B").to_ascii_uppercase();
    let val: f64 = num_str.parse().unwrap_or(0.0);
    let mul = match unit.as_str() {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (val * mul) as u64
}

/// Parse a ratio percentage string (e.g., "45%") to u64
pub fn parse_ratio_pct(s: &str) -> u64 {
    s.trim_end_matches('%').parse::<u64>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // parse_size_to_bytes
    // =========================================================================

    #[test]
    fn parse_size_bytes() {
        assert_eq!(parse_size_to_bytes("512 B"), 512);
    }

    #[test]
    fn parse_size_kilobytes() {
        assert_eq!(parse_size_to_bytes("1.0 KB"), 1024);
    }

    #[test]
    fn parse_size_megabytes() {
        assert_eq!(parse_size_to_bytes("2.5 MB"), (2.5 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn parse_size_gigabytes() {
        assert_eq!(
            parse_size_to_bytes("1.0 GB"),
            (1024.0 * 1024.0 * 1024.0) as u64
        );
    }

    #[test]
    fn parse_size_lowercase_unit() {
        // to_ascii_uppercase handles this
        assert_eq!(parse_size_to_bytes("10 kb"), 10 * 1024);
    }

    #[test]
    fn parse_size_no_unit_defaults_to_bytes() {
        assert_eq!(parse_size_to_bytes("42"), 42);
    }

    #[test]
    fn parse_size_empty_string() {
        assert_eq!(parse_size_to_bytes(""), 0);
    }

    #[test]
    fn parse_size_invalid_number() {
        assert_eq!(parse_size_to_bytes("abc KB"), 0);
    }

    // =========================================================================
    // parse_ratio_pct
    // =========================================================================

    #[test]
    fn parse_ratio_with_percent_sign() {
        assert_eq!(parse_ratio_pct("45%"), 45);
    }

    #[test]
    fn parse_ratio_without_percent_sign() {
        assert_eq!(parse_ratio_pct("80"), 80);
    }

    #[test]
    fn parse_ratio_zero() {
        assert_eq!(parse_ratio_pct("0%"), 0);
    }

    #[test]
    fn parse_ratio_hundred() {
        assert_eq!(parse_ratio_pct("100%"), 100);
    }

    #[test]
    fn parse_ratio_invalid() {
        assert_eq!(parse_ratio_pct("abc"), 0);
    }

    #[test]
    fn parse_ratio_empty() {
        assert_eq!(parse_ratio_pct(""), 0);
    }

    // =========================================================================
    // SortState default
    // =========================================================================

    #[test]
    fn sort_state_default() {
        let state = SortState::default();
        assert_eq!(state.column, SortColumn::Name);
        assert!(state.ascending);
    }
}
