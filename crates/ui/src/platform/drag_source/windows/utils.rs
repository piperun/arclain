use arclain_core::ArchiveBackend;
use std::sync::Arc;
use tracing::{debug, warn};

/// Register string format to get u16 ID
pub fn get_clipboard_format(name: &str) -> u16 {
    use windows::core::PCWSTR;
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        windows::Win32::System::DataExchange::RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) as u16
    }
}

/// Find the common directory that contains all the given file paths.
/// Returns None if files are in different root directories or if the list is empty.
pub fn find_common_directory(file_paths: &[String]) -> Option<String> {
    if file_paths.is_empty() {
        return None;
    }

    // Normalize paths to use forward slashes
    let normalized: Vec<String> = file_paths.iter().map(|p| p.replace('\\', "/")).collect();

    // Get the first path's directory components
    let first = &normalized[0];
    let first_parts: Vec<&str> = first.split('/').collect();

    // If first path has no directory part, check if all paths are in root
    if first_parts.len() <= 1 {
        // All files must be in root (no directory part)
        let all_in_root = normalized.iter().all(|p| !p.contains('/'));
        return if all_in_root {
            Some(String::new())
        } else {
            None
        };
    }

    // Find the longest common directory prefix
    let mut common_parts = &first_parts[..first_parts.len() - 1]; // Exclude filename

    for path in normalized.iter().skip(1) {
        let parts: Vec<&str> = path.split('/').collect();
        let dir_parts = &parts[..parts.len().saturating_sub(1)];

        // Find how many parts match
        let mut match_count = 0;
        for (i, part) in common_parts.iter().enumerate() {
            if i < dir_parts.len() && dir_parts[i] == *part {
                match_count += 1;
            } else {
                break;
            }
        }

        // Shrink common_parts to the matching portion
        common_parts = &common_parts[..match_count];

        if common_parts.is_empty() {
            return None;
        }
    }

    if common_parts.is_empty() {
        None
    } else {
        Some(common_parts.join("/"))
    }
}

/// Threshold for when to use extract_all vs extract_files
pub const MAX_FILES_FOR_EXTRACT_FILES: usize = 75;

/// Extract files with a native Windows progress dialog.
pub fn extract_with_progress_dialog(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: &std::path::Path,
    dest_dir: &std::path::Path,
    file_paths: &[String],
    password: Option<&str>,
) -> std::result::Result<(), String> {
    let file_count = file_paths.len();

    // For very small file counts (1-2 files), just extract directly without dialog
    if file_count <= 2 {
        debug!(
            "[drag] Small file count ({}), extracting without progress dialog",
            file_count
        );
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    }

    // For large file counts, use extract_all to avoid command line length limits
    let use_extract_all = file_count > MAX_FILES_FOR_EXTRACT_FILES;
    if use_extract_all {
        debug!(
            "[drag] Large file count ({}), will use extract_all to avoid command line limits",
            file_count
        );

        // Find the common directory prefix for all files
        let common_dir = find_common_directory(file_paths);

        if let Some(dir_path) = common_dir {
            debug!(
                "[drag] Using extract_directory with pattern: {}/*",
                dir_path
            );
            return backend
                .extract_directory(archive_path, dest_dir, &dir_path, password)
                .map_err(|e| {
                    warn!("[drag] extract_directory error: {}", e);
                    format!("Extraction failed: {}", e)
                });
        } else {
            debug!("[drag] No common directory found, using extract_all");
            return backend
                .extract_all(archive_path, dest_dir, password)
                .map_err(|e| {
                    warn!("[drag] extract_all error: {}", e);
                    format!("Extraction failed: {}", e)
                });
        }
    }

    // Use native Windows IProgressDialog for extraction with progress
    debug!(
        "[drag] Starting extraction with native Windows progress dialog for {} files",
        file_count
    );
    // Note: native_progress is in the parent directory.
    // We cannot easily access parent module private items if they are not pub.
    // native_progress should be accessible via crate::platform::drag_source::native_progress
    use crate::platform::drag_source::native_progress;
    
    native_progress::extract_with_native_progress(
        backend,
        archive_path,
        dest_dir,
        file_paths,
        password,
    )
}
