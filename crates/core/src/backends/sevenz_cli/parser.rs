//! Output parsing for 7-Zip CLI

use super::SevenZipCli;
use crate::{ArchiveEntry, ArchiveInfo, ArchiveKind};
use anyhow::Result;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::Path;
use tracing::{debug, info};

impl SevenZipCli {
    /// Parse archive type from 7z -slt output
    pub(crate) fn parse_kind(slt: &str) -> ArchiveKind {
        for line in slt.lines() {
            if let Some(rest) = line.strip_prefix("Type = ") {
                return match rest.trim().to_lowercase().as_str() {
                    "zip" => ArchiveKind::Zip,
                    "7z" => ArchiveKind::SevenZ,
                    "rar" => ArchiveKind::Rar,
                    other => ArchiveKind::Unknown(other.to_string()),
                };
            }
        }
        ArchiveKind::Unknown("unknown".into())
    }

    /// Check if a RAR archive has encrypted contents without attempting password.
    /// This is useful because 7z cannot handle RAR encryption perfectly, so we want
    /// to detect encryption status upfront before attempting extraction.
    /// Returns true if any files are encrypted (not just headers).
    pub fn is_rar_encrypted(&self, path: &Path) -> Result<bool> {
        info!("Checking RAR encryption status: {}", path.display());

        let args = vec![
            OsString::from("l"),
            OsString::from("-ba"),
            OsString::from("-slt"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
            OsString::from("-p"), // Empty password to avoid interactive prompt
            path.as_os_str().to_os_string(),
        ];

        // Run the command - it will fail on encrypted headers but may succeed on encrypted files
        let result = self.run(args);

        match result {
            Ok(output) => {
                // Parse the output to check for encrypted entries
                let has_encrypted_files = output.lines().any(|line| {
                    line.starts_with("Encrypted = +")
                        || (line.starts_with("Encrypted = ") && line.contains("+"))
                });

                debug!(
                    "RAR encryption check result: encrypted={}",
                    has_encrypted_files
                );
                Ok(has_encrypted_files)
            }
            Err(e) => {
                // If 7z fails, it might be due to encrypted headers
                let err_msg = e.to_string().to_lowercase();
                if err_msg.contains("wrong password")
                    || err_msg.contains("can not open encrypted archive")
                    || err_msg.contains("encrypted")
                {
                    debug!("RAR has encrypted headers or cannot open without password");
                    Ok(true)
                } else {
                    // Some other error occurred
                    Err(e)
                }
            }
        }
    }

    /// Parse 7z -slt output into ArchiveInfo
    pub(crate) fn parse_list_slt(&self, archive_path: &Path, slt: &str) -> ArchiveInfo {
        let mut entries = Vec::new();
        let mut cur: Vec<(String, String)> = Vec::new();
        let mut header_props: HashMap<String, String> = HashMap::new();
        let mut in_entries = false;
        let mut encrypted_methods: BTreeSet<String> = BTreeSet::new();

        let flush = |cur: &Vec<(String, String)>,
                     entries: &mut Vec<ArchiveEntry>,
                     encrypted_methods: &mut BTreeSet<String>| {
            if cur.is_empty() {
                return;
            }

            let mut map = HashMap::new();
            for (k, v) in cur {
                map.insert(k.as_str(), v.as_str());
            }

            let has_path = map.contains_key("Path");
            let has_attributes = map.contains_key("Attributes") || map.contains_key("Folder");

            if !has_path || !has_attributes {
                return;
            }

            let mut path = map.get("Path").unwrap_or(&"").to_string();
            if path.starts_with("./") {
                path = path[2..].to_string();
            }
            path = path.replace('\\', "/");
            if path.ends_with('/') && path.len() > 1 {
                path.pop();
                while path.ends_with('/') {
                    path.pop();
                }
            }

            let is_dir = match map.get("Folder") {
                Some(&"+") => true,
                _ => match map.get("Attributes") {
                    Some(attrs) if attrs.contains('D') => true,
                    _ => false,
                },
            };

            let size = map
                .get("Size")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let packed = map
                .get("Packed Size")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            // Normalized, never passed through: 7-Zip prints a dot and
            // seven sub-second digits, which is not the shape a consumer
            // of `ArchiveEntry::modified` parses -- so passing the text
            // along left every entry this tier listed dateless. A value
            // that does not parse yields no time rather than a string
            // that reads as populated and resolves to nothing.
            let modified = map
                .get("Modified")
                .and_then(|s| crate::backends::entry_time::from_cli_text(s));
            let crc32 = map
                .get("CRC")
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty());
            let encrypted = matches!(map.get("Encrypted"), Some(&"+"));

            if encrypted {
                if let Some(method) = map.get("Method") {
                    if !method.trim().is_empty() {
                        encrypted_methods.insert(method.trim().to_string());
                    }
                }
            }

            entries.push(ArchiveEntry {
                path,
                size,
                packed_size: packed,
                modified,
                is_dir,
                encrypted,
                crc32,
            });
        };

        for line in slt.lines() {
            let line = line.trim_end();

            if line.starts_with("----------") {
                in_entries = true;
                continue;
            }

            // Key/value line
            if let Some((k, v)) = line.split_once(" = ") {
                let key = k.trim();
                let value = v.trim();

                // Starting a new entry block on Path
                if key == "Path" {
                    if !cur.is_empty() {
                        flush(&cur, &mut entries, &mut encrypted_methods);
                        cur.clear();
                    }
                    in_entries = true;
                    cur.push((key.to_string(), value.to_string()));
                    continue;
                }

                // Header-level keys that may appear after entries too
                let is_header_key = matches!(
                    key,
                    "Headers Encrypted"
                        | "Encryption"
                        | "Encrypted"
                        | "Header Encryption"
                        | "Characteristics"
                );

                if in_entries && !cur.is_empty() && !is_header_key {
                    // Entry field while inside an entry block
                    cur.push((key.to_string(), value.to_string()));
                } else {
                    // Treat as header property (also captures footer header lines)
                    header_props.insert(key.to_string(), value.to_string());
                }
                continue;
            }

            // Empty line: end current entry block if any
            if line.is_empty() {
                if !cur.is_empty() {
                    flush(&cur, &mut entries, &mut encrypted_methods);
                    cur.clear();
                }
                continue;
            }
        }

        // Don't forget to flush the last entry
        flush(&cur, &mut entries, &mut encrypted_methods);

        let mut archive_encrypted = entries.iter().any(|entry| entry.encrypted);

        if let Some(value) = header_props.get("Encrypted") {
            if value == "+" || value.eq_ignore_ascii_case("yes") {
                archive_encrypted = true;
            }
        }

        if let Some(value) = header_props.get("Encryption") {
            if !value.trim().is_empty() {
                archive_encrypted = true;
                encrypted_methods.insert(value.trim().to_string());
            }
        }

        if let Some(value) = header_props.get("Characteristics") {
            if value.to_lowercase().contains("encrypted") {
                archive_encrypted = true;
            }
        }

        // Detect header encryption across variants
        let mut headers_encrypted = matches!(
            header_props.get("Headers Encrypted"),
            Some(value) if value == "+" || value.eq_ignore_ascii_case("yes")
        );

        // Some formats expose explicit header encryption method without a boolean flag
        if let Some(value) = header_props.get("Header Encryption") {
            if !value.trim().is_empty() {
                headers_encrypted = true;
                encrypted_methods.insert(value.trim().to_string());
            }
        }

        // Fallback: some variants put hints in Characteristics
        if let Some(value) = header_props.get("Characteristics") {
            let lc = value.to_lowercase();
            if lc.contains("headers encrypted") || lc.contains("encrypted headers") {
                headers_encrypted = true;
            }
        }

        let encryption_method = if archive_encrypted || headers_encrypted {
            if !encrypted_methods.is_empty() {
                Some(encrypted_methods.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                header_props.get("Method").cloned()
            }
        } else {
            None
        };

        let kind = Self::parse_kind(slt);
        ArchiveInfo {
            archive_path: archive_path.to_path_buf(),
            archive_kind: kind,
            entries,
            encrypted: archive_encrypted,
            headers_encrypted,
            encryption_method,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(slt: &str) -> ArchiveInfo {
        SevenZipCli {
            exe: PathBuf::from("7z"),
        }
        .parse_list_slt(Path::new("timestamped.7z"), slt)
    }

    /// A verbatim `7z l -slt` entry block, captured from 7-Zip 26.02.
    /// The `Modified` value carries a dot and seven sub-second digits,
    /// which is not the shape consumers parse -- passing it through left
    /// every entry from this tier dateless.
    #[test]
    fn a_real_slt_block_yields_a_time_in_the_shape_consumers_parse() {
        let info = parse(
            "\
----------
Path = timestamped.txt
Size = 29
Packed Size = 33
Modified = 2026-05-04 13:37:20.0000000
Attributes = A_ -rw-rw-rw-
CRC = 71C29FC3
Encrypted = -
Method = LZMA2:12
",
        );

        let entry = info
            .entries
            .iter()
            .find(|entry| entry.path == "timestamped.txt")
            .expect("the block's entry is parsed");
        assert_eq!(entry.modified.as_deref(), Some("2026-05-04 13:37:20"));
    }

    /// A directory entry carries a time of its own, in the same shape,
    /// and must be normalized the same way rather than only files being
    /// handled.
    #[test]
    fn a_directory_entrys_time_is_normalized_too() {
        let info = parse(
            "\
----------
Path = emptydir
Size = 0
Modified = 2026-08-07 19:47:58.5437627
Attributes = D
",
        );

        assert!(info.entries[0].is_dir, "control: the entry is a directory");
        assert_eq!(
            info.entries[0].modified.as_deref(),
            Some("2026-08-07 19:47:58")
        );
    }

    /// A value this parser cannot make sense of -- a truncated line, or a
    /// future 7-Zip printing something else entirely -- must report no
    /// time. Reporting the raw text instead is what left this tier's
    /// entries dateless while looking populated.
    #[test]
    fn a_value_that_does_not_parse_reports_none() {
        let info = parse(
            "\
----------
Path = odd.txt
Size = 1
Modified = not a timestamp
Attributes = A
",
        );

        assert_eq!(info.entries[0].modified, None);
    }
}
