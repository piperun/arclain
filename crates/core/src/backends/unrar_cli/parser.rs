//! Output parsing for UnRAR CLI

use super::UnrarCli;
use crate::{ArchiveEntry, ArchiveInfo, ArchiveKind};
use std::path::Path;

/// The value of a `vt` block's modification-time line, whichever of
/// unrar's labels this build prints.
///
/// It varies by platform and by the tool's own message catalogue --
/// Windows builds print `Modified:` (which this parser missed entirely
/// until it was checked against real output), other builds print
/// `mtime:`, and older ones `Time:` or `Last write time:`. All four are
/// accepted rather than guessing which one a given machine will produce.
fn modification_time_field(line: &str) -> Option<&str> {
    const LABELS: [&str; 4] = ["Modified: ", "mtime: ", "Time: ", "Last write time: "];
    LABELS
        .iter()
        .find_map(|label| line.strip_prefix(label))
        .map(str::trim)
}

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
                } else if let Some(printed) = modification_time_field(line) {
                    // Whatever unrar printed is normalized, never passed
                    // through: its own format carries a comma and nine
                    // sub-second digits that no consumer of
                    // `ArchiveEntry::modified` accepts.
                    entry.modified = crate::backends::entry_time::from_cli_text(printed);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A verbatim `unrar vt` block, captured from UnRAR 7.23 on Windows
    /// listing a RAR4 archive. Both details this parser used to get wrong
    /// are visible in it: the label is `Modified`, not `mtime`, and the
    /// value carries a comma and nine sub-second digits.
    const REAL_VT_OUTPUT: &str = "\
UNRAR 7.23 x64 freeware      Copyright (c) 1993-2026 Alexander Roshal

Archive: timestamped.rar
Details: RAR 1.5

        Name: timestamped.txt
        Type: File
        Size: 30
 Packed size: 30
       Ratio: 100%
    Modified: 2026-05-04 13:37:20,000000000
  Attributes: ..A....
       CRC32: 71C29FC3
     Host OS: Windows
 Compression: RAR 1.5(v20) -m0 -md=64k
";

    fn parse(output: &str) -> ArchiveInfo {
        UnrarCli {
            exe: PathBuf::from("unrar"),
        }
        .parse_list_output(Path::new("timestamped.rar"), output)
    }

    /// The CLI tier is what serves a RAR whose native listing failed, so
    /// a time it drops is a time the user never sees. This parser dropped
    /// every one of them: the label it looked for is not the label this
    /// build prints, and the value it would have kept is a shape
    /// consumers reject.
    ///
    /// What unrar prints is its own local-zone rendering of the entry's
    /// time, so the expectation is that wall clock put through the
    /// wall-clock policy in `crate::backends::entry_time` -- derived via
    /// `chrono::Local`, which keeps the assertion zone-stable.
    #[test]
    fn a_real_vt_block_yields_a_time_in_the_shape_consumers_parse() {
        let expected = crate::backends::entry_time::test_support::utc_wire_string_of_local_wall(
            2026, 5, 4, 13, 37, 20,
        );
        let info = parse(REAL_VT_OUTPUT);

        let entry = info
            .entries
            .iter()
            .find(|entry| entry.path == "timestamped.txt")
            .expect("the block's entry is parsed");
        assert_eq!(entry.modified.as_deref(), Some(expected.as_str()));
    }

    /// unrar's label differs by platform and message catalogue, so every
    /// spelling it is known to print must resolve to the same value.
    #[test]
    fn every_label_this_tool_is_known_to_print_is_recognized() {
        let expected = crate::backends::entry_time::test_support::utc_wire_string_of_local_wall(
            2026, 5, 4, 13, 37, 20,
        );
        for label in ["Modified", "mtime", "Time", "Last write time"] {
            let output = format!(
                "        Name: a.txt\n        Type: File\n  {label}: 2026-05-04 13:37:20\n"
            );
            let info = parse(&output);
            assert_eq!(
                info.entries[0].modified.as_deref(),
                Some(expected.as_str()),
                "the {label:?} spelling must be recognized"
            );
        }
    }

    /// A block with no time line, or one this parser cannot make sense
    /// of, must report no time -- never a half-parsed string that reads
    /// as real and resolves to nothing.
    #[test]
    fn a_block_with_no_usable_time_reports_none() {
        let no_line = parse("        Name: a.txt\n        Type: File\n        Size: 1\n");
        assert_eq!(no_line.entries[0].modified, None);

        let unusable = parse("        Name: a.txt\n    Modified: not a timestamp\n");
        assert_eq!(unusable.entries[0].modified, None);
    }
}
