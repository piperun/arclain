//! External file drop handling
//!
//! Handles files dropped onto the Arclain window from the system file manager.

use eframe::egui;
use std::path::PathBuf;

/// Supported archive extensions for drop handling
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];

/// Result of processing dropped files
pub enum DropAction {
    /// No files dropped or dropped file not supported
    None,
    /// Open the dropped archive
    OpenArchive(PathBuf),
}

/// Check if a path has a supported archive extension
fn is_archive(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| ARCHIVE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Process files dropped onto the app window
///
/// Returns a `DropAction` indicating what to do with the dropped file.
/// Only processes the first dropped file if multiple are dropped.
pub fn process_dropped_files(ctx: &egui::Context) -> DropAction {
    // Ignore drops while we're doing an outgoing drag (our own data)
    #[cfg(target_os = "windows")]
    if crate::platform::drag_source::is_outgoing_drag_active() {
        return DropAction::None;
    }

    ctx.input(|i| {
        if let Some(file) = i.raw.dropped_files.first() {
            if let Some(path) = &file.path {
                if is_archive(path) {
                    tracing::info!("Archive dropped: {}", path.display());
                    return DropAction::OpenArchive(path.clone());
                } else {
                    tracing::debug!(
                        "Dropped file is not a supported archive: {}",
                        path.display()
                    );
                }
            }
        }
        DropAction::None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_archive() {
        assert!(is_archive(std::path::Path::new("test.zip")));
        assert!(is_archive(std::path::Path::new("test.7z")));
        assert!(is_archive(std::path::Path::new("test.rar")));
        assert!(is_archive(std::path::Path::new("test.tar.gz")));
        assert!(!is_archive(std::path::Path::new("test.txt")));
        assert!(!is_archive(std::path::Path::new("test.exe")));
    }
}
