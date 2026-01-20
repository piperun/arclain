//! File entry data models used across features

/// Represents a single file or folder entry in the file list
#[derive(Clone, Debug)]
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
#[derive(Debug, Clone, Copy)]
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
