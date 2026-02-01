//! Output parsing for UnRAR CLI

use super::UnrarCli;
use crate::{ArchiveEntry, ArchiveInfo, ArchiveKind};
use std::path::Path;

impl UnrarCli {
    /// Parse unrar listing output (v or vt command)
    pub(crate) fn parse_list_output(&self, archive_path: &Path, output: &str) -> ArchiveInfo {
        let mut entries = Vec::new();
        let mut encrypted = false;
        let mut headers_encrypted = false;

        // UnRAR vt output format has blocks like:
        //   Name: filename
        //   Type: File
        //   Size: 12345
        //   Packed size: 6789
        //   ...

        let mut current_entry: Option<ArchiveEntry> = None;

        // Helper to parse numbers that might contain commas or spaces
        let parse_number = |s: &str| -> u64 {
            let clean: String = s.chars().filter(|c| c.is_digit(10)).collect();
            clean.parse().unwrap_or(0)
        };

        for line in output.lines() {
            let line = line.trim();

            if line.starts_with("Name: ") {
                // Flush previous entry
                if let Some(entry) = current_entry.take() {
                    entries.push(entry);
                }

                let path = line.strip_prefix("Name: ").unwrap_or("").to_string();
                current_entry = Some(ArchiveEntry {
                    path,
                    size: 0,
                    packed_size: 0,
                    modified: None,
                    is_dir: false,
                    encrypted: false,
                    crc32: None,
                });
            } else if let Some(ref mut entry) = current_entry {
                if line.starts_with("Type: ") {
                    entry.is_dir = line.contains("Directory") || line.contains("Dir");
                } else if line.starts_with("Size: ") {
                    if let Some(s) = line.strip_prefix("Size: ") {
                        entry.size = parse_number(s);
                    }
                } else if line.starts_with("Packed size: ") {
                    if let Some(s) = line.strip_prefix("Packed size: ") {
                        entry.packed_size = parse_number(s);
                    }
                } else if line.starts_with("mtime: ") {
                    entry.modified = line.strip_prefix("mtime: ").map(|s| s.trim().to_string());
                } else if line.starts_with("Time: ") {
                    entry.modified = line.strip_prefix("Time: ").map(|s| s.trim().to_string());
                } else if line.starts_with("Last write time: ") {
                    entry.modified = line
                        .strip_prefix("Last write time: ")
                        .map(|s| s.trim().to_string());
                } else if line.starts_with("CRC32: ") {
                    entry.crc32 = line
                        .strip_prefix("CRC32: ")
                        .map(|s| s.trim().to_uppercase());
                } else if line.starts_with("Flags: ") && line.contains("encrypted") {
                    entry.encrypted = true;
                    encrypted = true;
                }
            }

            // Check for header encryption indicators
            if line.contains("encrypted headers") || line.contains("Encrypted headers") {
                headers_encrypted = true;
                encrypted = true;
            }
        }

        // Flush last entry
        if let Some(entry) = current_entry {
            entries.push(entry);
        }

        ArchiveInfo {
            archive_path: archive_path.to_path_buf(),
            archive_kind: ArchiveKind::Rar,
            entries,
            encrypted,
            headers_encrypted,
            encryption_method: if encrypted {
                Some("RAR".to_string())
            } else {
                None
            },
        }
    }
}
