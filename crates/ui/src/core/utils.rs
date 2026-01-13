use crate::shared::components::file_list::FileEntry;
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
        .map(|n| n.to_string_lossy().to_string())
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
}
