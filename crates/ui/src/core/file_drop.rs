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

/// Drop the duplicates from a dropped-path list, preserving first-seen
/// order.
///
/// Some platforms (observed on Windows via winit) occasionally deliver
/// a multi-file drop gesture with the final file repeated — the drop
/// overlay would then open that archive into two tabs and fire
/// `OnArchiveOpen` twice for it. The exact upstream cause varies by
/// platform/compositor, so we guard defensively here rather than chase
/// it through winit: a single drop gesture should never open the same
/// path more than once.
pub fn dedupe_dropped_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Process files dropped onto the app window
///
/// Returns a `DropAction` indicating what to do with the dropped file.
/// Only processes the first dropped file if multiple are dropped.
pub fn process_dropped_files(ctx: &egui::Context) -> DropAction {
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

    #[test]
    fn dedupe_keeps_first_seen_order() {
        let paths = vec![
            PathBuf::from("a.zip"),
            PathBuf::from("b.rar"),
            PathBuf::from("c.7z"),
        ];
        assert_eq!(dedupe_dropped_paths(paths.clone()), paths);
    }

    #[test]
    fn dedupe_drops_trailing_repeat() {
        // The exact failure mode from the multi-drop bug: the last
        // archive delivered twice by the platform drop event.
        let paths = vec![
            PathBuf::from("a.zip"),
            PathBuf::from("b.rar"),
            PathBuf::from("c.7z"),
            PathBuf::from("c.7z"),
        ];
        assert_eq!(
            dedupe_dropped_paths(paths),
            vec![
                PathBuf::from("a.zip"),
                PathBuf::from("b.rar"),
                PathBuf::from("c.7z"),
            ]
        );
    }

    #[test]
    fn dedupe_drops_interior_and_repeated_duplicates() {
        let paths = vec![
            PathBuf::from("a.zip"),
            PathBuf::from("b.rar"),
            PathBuf::from("a.zip"),
            PathBuf::from("b.rar"),
        ];
        assert_eq!(
            dedupe_dropped_paths(paths),
            vec![PathBuf::from("a.zip"), PathBuf::from("b.rar")]
        );
    }
}
