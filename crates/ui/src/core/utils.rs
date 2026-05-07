use crate::shared::models::file_entry::FileEntry;
use tracing::error;

pub fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", size as u64, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

pub fn convert_to_file_entry(entry: &arclain_core::ArchiveEntry) -> FileEntry {
    let ratio = if entry.size > 0 {
        format!("{}%", (entry.packed_size * 100 / entry.size))
    } else {
        "0%".to_string()
    };

    // Extract just the filename/folder name from the full path
    let name = std::path::Path::new(&entry.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.path.clone());

    FileEntry {
        name,
        path: entry.path.clone(),
        size: format_size(entry.size),
        compressed: format_size(entry.packed_size),
        ratio,
        modified: entry.modified.clone().unwrap_or_default(),
        crc32: entry.crc32.clone().unwrap_or_default(),
        encrypted: entry.encrypted,
        is_folder: entry.is_dir,
        selected: false,
    }
}

/// Log an error in a consistent format for failure cases.
/// This keeps our tests simple and ensures a single message shape.
pub fn log_failure(context: &str, message: impl std::fmt::Display) {
    error!("{}: {}", context, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_test::traced_test;

    // Verifies that our failure logging helper actually emits a log line we can assert on.
    #[traced_test]
    #[test]
    fn logs_on_failure() {
        log_failure("Settings", "failed to save");
        assert!(logs_contain("Settings: failed to save"));
    }

    // =========================================================================
    // format_duration
    // =========================================================================

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(std::time::Duration::from_secs(0)), "00:00");
    }

    #[test]
    fn format_duration_seconds_only() {
        assert_eq!(format_duration(std::time::Duration::from_secs(45)), "00:45");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(125)),
            "02:05"
        );
    }

    #[test]
    fn format_duration_exactly_one_hour() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(3600)),
            "01:00:00"
        );
    }

    #[test]
    fn format_duration_hours_minutes_seconds() {
        // 2h 30m 15s = 9015s
        assert_eq!(
            format_duration(std::time::Duration::from_secs(9015)),
            "02:30:15"
        );
    }

    // =========================================================================
    // format_size
    // =========================================================================

    #[test]
    fn format_size_zero_bytes() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn format_size_gigabytes() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_size_fractional_mb() {
        // 1.5 MB = 1572864 bytes
        assert_eq!(format_size(1_572_864), "1.5 MB");
    }

    // =========================================================================
    // convert_to_file_entry
    // =========================================================================

    #[test]
    fn convert_to_file_entry_file() {
        let entry = arclain_core::ArchiveEntry {
            path: "game/data/save.dat".to_string(),
            size: 2048,
            packed_size: 1024,
            modified: Some("2024-01-15".to_string()),
            is_dir: false,
            encrypted: false,
            crc32: Some("AABBCCDD".to_string()),
        };
        let fe = convert_to_file_entry(&entry);
        assert_eq!(fe.name, "save.dat");
        assert_eq!(fe.path, "game/data/save.dat");
        assert_eq!(fe.ratio, "50%");
        assert_eq!(fe.modified, "2024-01-15");
        assert_eq!(fe.crc32, "AABBCCDD");
        assert!(!fe.is_folder);
        assert!(!fe.encrypted);
        assert!(!fe.selected);
    }

    #[test]
    fn convert_to_file_entry_folder() {
        let entry = arclain_core::ArchiveEntry {
            path: "game/data".to_string(),
            size: 0,
            packed_size: 0,
            modified: None,
            is_dir: true,
            encrypted: false,
            crc32: None,
        };
        let fe = convert_to_file_entry(&entry);
        assert_eq!(fe.name, "data");
        assert!(fe.is_folder);
        assert_eq!(fe.ratio, "0%");
        assert_eq!(fe.modified, "");
        assert_eq!(fe.crc32, "");
    }

    #[test]
    fn convert_to_file_entry_encrypted() {
        let entry = arclain_core::ArchiveEntry {
            path: "secret.txt".to_string(),
            size: 100,
            packed_size: 80,
            modified: None,
            is_dir: false,
            encrypted: true,
            crc32: None,
        };
        let fe = convert_to_file_entry(&entry);
        assert!(fe.encrypted);
        assert_eq!(fe.ratio, "80%");
    }
}
