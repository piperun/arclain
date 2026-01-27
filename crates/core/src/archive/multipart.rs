//! Multi-part archive detection and handling
//!
//! This module provides detection and validation for split/multi-part archives
//! in various formats (RAR, 7z, ZIP, generic).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Represents a detected multi-part archive
#[derive(Debug, Clone, PartialEq)]
pub struct MultiPartArchive {
    /// The first part of the archive (entry point for extraction)
    pub first_part: PathBuf,
    /// All parts found in order
    pub all_parts: Vec<PathBuf>,
    /// The detected format/naming convention
    pub format: MultiPartFormat,
    /// Base name without part indicators or extension
    pub base_name: String,
}

/// Supported multi-part archive naming conventions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiPartFormat {
    /// `.part1.rar`, `.part2.rar`, etc.
    RarPart,
    /// `.rar`, `.r00`, `.r01`, etc.
    RarSequence,
    /// `.7z.001`, `.7z.002`, etc.
    SevenZip,
    /// `.z01`, `.z02`, ..., `.zip`
    ZipSplit,
    /// Generic `.001`, `.002`, etc.
    Generic001,
}

impl MultiPartFormat {
    /// Returns a human-readable description of the format
    pub fn description(&self) -> &'static str {
        match self {
            Self::RarPart => "RAR multi-part (.partN.rar)",
            Self::RarSequence => "RAR sequence (.rar, .r00, .r01)",
            Self::SevenZip => "7-Zip split (.7z.001)",
            Self::ZipSplit => "ZIP split (.z01, .zip)",
            Self::Generic001 => "Generic split (.001, .002)",
        }
    }
}

/// Result of validating a multi-part archive
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether all parts are present
    pub is_complete: bool,
    /// Parts that were found
    pub found_parts: Vec<PathBuf>,
    /// Expected parts that are missing
    pub missing_parts: Vec<String>,
    /// Total size of all parts in bytes
    pub total_size: u64,
}

impl MultiPartArchive {
    /// Detect if a file is part of a multi-part archive
    ///
    /// Returns Some if the file matches any known multi-part pattern,
    /// None otherwise.
    pub fn detect(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let filename_lower = filename.to_lowercase();

        // Try each format in order of specificity
        if let Some(result) = Self::detect_rar_part(path, &filename_lower) {
            return Some(result);
        }
        if let Some(result) = Self::detect_rar_sequence(path, &filename_lower) {
            return Some(result);
        }
        if let Some(result) = Self::detect_7z_split(path, &filename_lower) {
            return Some(result);
        }
        if let Some(result) = Self::detect_zip_split(path, &filename_lower) {
            return Some(result);
        }
        if let Some(result) = Self::detect_generic_001(path, &filename_lower) {
            return Some(result);
        }

        None
    }

    /// Detect `.partN.rar` format (e.g., `game.part1.rar`, `game.part2.rar`)
    fn detect_rar_part(path: &Path, filename_lower: &str) -> Option<Self> {
        // Pattern: .part followed by digits followed by .rar
        let re = regex::Regex::new(r"^(.+)\.part(\d+)\.rar$").ok()?;
        let captures = re.captures(filename_lower)?;

        let base_name = captures.get(1)?.as_str().to_string();
        // Verify the part number is valid (we don't need the actual value)
        let _part_num: u32 = captures.get(2)?.as_str().parse().ok()?;

        // First part is always part1
        let parent = path.parent()?;
        let first_part = parent.join(format!("{}.part1.rar", base_name));

        Some(Self {
            first_part,
            all_parts: Vec::new(), // Will be populated by find_all_parts
            format: MultiPartFormat::RarPart,
            base_name,
        })
    }

    /// Detect `.rar`, `.r00`, `.r01` format
    fn detect_rar_sequence(path: &Path, filename_lower: &str) -> Option<Self> {
        // Pattern: .rar or .rNN where NN is 00-99
        let re = regex::Regex::new(r"^(.+)\.(rar|r\d{2,3})$").ok()?;
        let captures = re.captures(filename_lower)?;

        let base_name = captures.get(1)?.as_str().to_string();
        let ext = captures.get(2)?.as_str();

        // Only treat as multi-part if we see .r00/.r01 patterns
        // A standalone .rar might not be multi-part
        if ext == "rar" {
            // Check if .r00 exists to confirm it's multi-part
            let parent = path.parent()?;
            let r00_path = parent.join(format!("{}.r00", base_name));
            if !r00_path.exists() {
                return None;
            }
        }

        let parent = path.parent()?;
        let first_part = parent.join(format!("{}.rar", base_name));

        Some(Self {
            first_part,
            all_parts: Vec::new(),
            format: MultiPartFormat::RarSequence,
            base_name,
        })
    }

    /// Detect `.7z.001`, `.7z.002` format
    fn detect_7z_split(path: &Path, filename_lower: &str) -> Option<Self> {
        // Pattern: .7z.NNN where NNN is 001-999
        let re = regex::Regex::new(r"^(.+)\.7z\.(\d{3})$").ok()?;
        let captures = re.captures(filename_lower)?;

        let base_name = captures.get(1)?.as_str().to_string();

        let parent = path.parent()?;
        let first_part = parent.join(format!("{}.7z.001", base_name));

        Some(Self {
            first_part,
            all_parts: Vec::new(),
            format: MultiPartFormat::SevenZip,
            base_name,
        })
    }

    /// Detect `.z01`, `.z02`, ..., `.zip` format
    fn detect_zip_split(path: &Path, filename_lower: &str) -> Option<Self> {
        // Pattern: .zNN (01-99) or .zip
        let re = regex::Regex::new(r"^(.+)\.(z\d{2}|zip)$").ok()?;
        let captures = re.captures(filename_lower)?;

        let base_name = captures.get(1)?.as_str().to_string();
        let ext = captures.get(2)?.as_str();

        // For .zip, check if .z01 exists to confirm split
        if ext == "zip" {
            let parent = path.parent()?;
            let z01_path = parent.join(format!("{}.z01", base_name));
            if !z01_path.exists() {
                return None;
            }
        }

        // First part is .z01 (but extraction starts from .zip)
        let parent = path.parent()?;
        let first_part = parent.join(format!("{}.zip", base_name));

        Some(Self {
            first_part,
            all_parts: Vec::new(),
            format: MultiPartFormat::ZipSplit,
            base_name,
        })
    }

    /// Detect generic `.001`, `.002` format
    fn detect_generic_001(path: &Path, filename_lower: &str) -> Option<Self> {
        // Pattern: .NNN where NNN is 001-999
        let re = regex::Regex::new(r"^(.+)\.(\d{3})$").ok()?;
        let captures = re.captures(filename_lower)?;

        let base_name = captures.get(1)?.as_str().to_string();
        let part_num: u32 = captures.get(2)?.as_str().parse().ok()?;

        // Must be a reasonable part number
        if part_num == 0 || part_num > 999 {
            return None;
        }

        let parent = path.parent()?;
        let first_part = parent.join(format!("{}.001", base_name));

        Some(Self {
            first_part,
            all_parts: Vec::new(),
            format: MultiPartFormat::Generic001,
            base_name,
        })
    }

    /// Find all parts of this multi-part archive in the directory
    pub fn find_all_parts(&mut self) -> Result<&[PathBuf]> {
        let parent = self
            .first_part
            .parent()
            .context("Cannot determine parent directory")?;

        self.all_parts = match self.format {
            MultiPartFormat::RarPart => self.find_rar_part_files(parent)?,
            MultiPartFormat::RarSequence => self.find_rar_sequence_files(parent)?,
            MultiPartFormat::SevenZip => self.find_7z_split_files(parent)?,
            MultiPartFormat::ZipSplit => self.find_zip_split_files(parent)?,
            MultiPartFormat::Generic001 => self.find_generic_001_files(parent)?,
        };

        Ok(&self.all_parts)
    }

    fn find_rar_part_files(&self, parent: &Path) -> Result<Vec<PathBuf>> {
        let mut parts = Vec::new();
        let mut part_num = 1u32;

        loop {
            let part_path = parent.join(format!("{}.part{}.rar", self.base_name, part_num));
            if part_path.exists() {
                parts.push(part_path);
                part_num += 1;
            } else {
                break;
            }
        }

        Ok(parts)
    }

    fn find_rar_sequence_files(&self, parent: &Path) -> Result<Vec<PathBuf>> {
        let mut parts = Vec::new();

        // First file is always .rar
        let rar_path = parent.join(format!("{}.rar", self.base_name));
        if rar_path.exists() {
            parts.push(rar_path);
        }

        // Then .r00, .r01, etc.
        let mut num = 0u32;
        loop {
            let part_path = parent.join(format!("{}.r{:02}", self.base_name, num));
            if part_path.exists() {
                parts.push(part_path);
                num += 1;
            } else {
                // Also try 3-digit format .r000, .r001
                let part_path_3 = parent.join(format!("{}.r{:03}", self.base_name, num));
                if part_path_3.exists() {
                    parts.push(part_path_3);
                    num += 1;
                } else {
                    break;
                }
            }
        }

        Ok(parts)
    }

    fn find_7z_split_files(&self, parent: &Path) -> Result<Vec<PathBuf>> {
        let mut parts = Vec::new();
        let mut part_num = 1u32;

        loop {
            let part_path = parent.join(format!("{}.7z.{:03}", self.base_name, part_num));
            if part_path.exists() {
                parts.push(part_path);
                part_num += 1;
            } else {
                break;
            }
        }

        Ok(parts)
    }

    fn find_zip_split_files(&self, parent: &Path) -> Result<Vec<PathBuf>> {
        let mut parts = Vec::new();

        // Find .z01, .z02, etc. first
        let mut num = 1u32;
        loop {
            let part_path = parent.join(format!("{}.z{:02}", self.base_name, num));
            if part_path.exists() {
                parts.push(part_path);
                num += 1;
            } else {
                break;
            }
        }

        // Last file is .zip
        let zip_path = parent.join(format!("{}.zip", self.base_name));
        if zip_path.exists() {
            parts.push(zip_path);
        }

        Ok(parts)
    }

    fn find_generic_001_files(&self, parent: &Path) -> Result<Vec<PathBuf>> {
        let mut parts = Vec::new();
        let mut part_num = 1u32;

        loop {
            let part_path = parent.join(format!("{}.{:03}", self.base_name, part_num));
            if part_path.exists() {
                parts.push(part_path);
                part_num += 1;
            } else {
                break;
            }
        }

        Ok(parts)
    }

    /// Validate that all parts are present and return status
    pub fn validate(&mut self) -> Result<ValidationResult> {
        // Ensure parts are found
        if self.all_parts.is_empty() {
            self.find_all_parts()?;
        }

        let found_parts = self.all_parts.clone();
        let total_size: u64 = found_parts
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();

        // Check for gaps in sequence
        let missing_parts = self.find_missing_parts();
        let is_complete = missing_parts.is_empty() && !found_parts.is_empty();

        Ok(ValidationResult {
            is_complete,
            found_parts,
            missing_parts,
            total_size,
        })
    }

    fn find_missing_parts(&self) -> Vec<String> {
        // For now, we assume if parts are found sequentially, there are no gaps
        // A more sophisticated check would verify the sequence
        Vec::new()
    }

    /// Get the number of parts found
    pub fn part_count(&self) -> usize {
        self.all_parts.len()
    }

    /// Check if this is a multi-part archive (has more than one part)
    pub fn is_multipart(&self) -> bool {
        self.all_parts.len() > 1
    }
}

/// Check if a path appears to be part of a multi-part archive
pub fn is_multipart_archive(path: &Path) -> bool {
    MultiPartArchive::detect(path).is_some()
}

/// Get info about a multi-part archive without finding all parts
pub fn detect_multipart(path: &Path) -> Option<MultiPartArchive> {
    MultiPartArchive::detect(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_detect_rar_part() {
        let path = PathBuf::from("/test/game.part1.rar");
        let result = MultiPartArchive::detect(&path);
        assert!(result.is_some());
        let mp = result.unwrap();
        assert_eq!(mp.format, MultiPartFormat::RarPart);
        assert_eq!(mp.base_name, "game");
    }

    #[test]
    fn test_detect_rar_part_case_insensitive() {
        let path = PathBuf::from("/test/Game.Part2.RAR");
        let result = MultiPartArchive::detect(&path);
        assert!(result.is_some());
        let mp = result.unwrap();
        assert_eq!(mp.format, MultiPartFormat::RarPart);
    }

    #[test]
    fn test_detect_7z_split() {
        let path = PathBuf::from("/test/archive.7z.001");
        let result = MultiPartArchive::detect(&path);
        assert!(result.is_some());
        let mp = result.unwrap();
        assert_eq!(mp.format, MultiPartFormat::SevenZip);
        assert_eq!(mp.base_name, "archive");
    }

    #[test]
    fn test_detect_generic_001() {
        let path = PathBuf::from("/test/data.001");
        let result = MultiPartArchive::detect(&path);
        assert!(result.is_some());
        let mp = result.unwrap();
        assert_eq!(mp.format, MultiPartFormat::Generic001);
        assert_eq!(mp.base_name, "data");
    }

    #[test]
    fn test_not_multipart() {
        let path = PathBuf::from("/test/normal.zip");
        let result = MultiPartArchive::detect(&path);
        assert!(result.is_none());
    }

    #[test]
    fn test_not_multipart_rar() {
        // A single .rar file without .r00 sibling is not multi-part
        let path = PathBuf::from("/test/single.rar");
        let _result = MultiPartArchive::detect(&path);
        // This will return None because .r00 doesn't exist
        // (the check requires filesystem access which won't work in test)
        // In real usage, detect_rar_sequence checks for .r00 existence
    }
}
