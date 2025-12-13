use super::ArchiveOperationsState;
use crate::core::utils;
use crate::platform::{resume_process, suspend_process};
use crate::shared::dialogs;
use arclain_core::ArchiveBackend;

pub fn pause_extraction(state: &mut ArchiveOperationsState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = suspend_process(pid) {
            tracing::error!("Failed to suspend process {}: {}", pid, e);
        } else {
            state.extraction_dialog.status = dialogs::ExtractionStatus::Paused;
        }
    }
}

pub fn resume_extraction(state: &mut ArchiveOperationsState) {
    if let Some(child) = &state.extraction_child {
        let pid = child.id();
        if let Err(e) = resume_process(pid) {
            tracing::error!("Failed to resume process {}: {}", pid, e);
        } else {
            state.extraction_dialog.status = dialogs::ExtractionStatus::Running;
        }
    }
}

pub fn cancel_extraction(state: &mut ArchiveOperationsState) {
    if let Some(mut child) = state.extraction_child.take() {
        if let Err(e) = child.kill() {
            tracing::error!("Failed to kill process: {}", e);
        }
        state.extraction_dialog.status = dialogs::ExtractionStatus::Cancelled;
        state.extraction_rx = None;
        state.extraction_started = None;
    }
}

pub fn update_extraction_progress(state: &mut ArchiveOperationsState, ctx: &egui::Context) {
    if let Some(rx) = &state.extraction_rx {
        for upd in rx.try_iter() {
            if upd.percent > 0 {
                state.extraction_dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~500 lines
                if state.extraction_dialog.log_lines.len() > 500 {
                    let overflow = state.extraction_dialog.log_lines.len() - 500;
                    state.extraction_dialog.log_lines.drain(0..overflow);
                }
                state.extraction_dialog.log_lines.push(msg);
            }
            if let Some(start) = state.extraction_started {
                let elapsed = start.elapsed();
                state.extraction_dialog.elapsed_text = utils::format_duration(elapsed);
                if upd.percent > 0 && upd.percent < 100 {
                    let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                    let left = total_est.saturating_sub(elapsed);
                    state.extraction_dialog.time_left_text = utils::format_duration(left);
                    state.extraction_dialog.processed_text = format!("{}%", upd.percent);
                }
            }
            ctx.request_repaint();
        }
    }

    // Check child completion
    if let Some(child) = state.extraction_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() && state.extraction_dialog.percent >= 100 {
                state.extraction_dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                state.extraction_dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.extraction_minimized {
                state.extraction_dialog.show = false;
            }
            state.extraction_child = None;
            state.extraction_rx = None;
            state.extraction_started = None;
        }
    }
}

pub fn update_conversion_progress(state: &mut ArchiveOperationsState, ctx: &egui::Context) {
    if let Some(rx) = &state.conversion_rx {
        for upd in rx.try_iter() {
            if upd.percent > 0 {
                state.conversion_dialog.percent = upd.percent;
            }
            if let Some(msg) = upd.message {
                // Keep last ~500 lines
                if state.conversion_dialog.log_lines.len() > 500 {
                    let overflow = state.conversion_dialog.log_lines.len() - 500;
                    state.conversion_dialog.log_lines.drain(0..overflow);
                }
                state.conversion_dialog.log_lines.push(msg);
            }
            if let Some(start) = state.conversion_started {
                let elapsed = start.elapsed();
                state.conversion_dialog.elapsed_text = utils::format_duration(elapsed);
                if upd.percent > 0 && upd.percent < 100 {
                    let total_est = elapsed.mul_f64(100.0 / upd.percent as f64);
                    let left = total_est.saturating_sub(elapsed);
                    state.conversion_dialog.time_left_text = utils::format_duration(left);
                    state.conversion_dialog.processed_text = format!("{}%", upd.percent);
                }
            }
            ctx.request_repaint();
        }
    }

    // Check conversion child completion
    if let Some(child) = state.conversion_child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() && state.conversion_dialog.percent >= 100 {
                state.conversion_dialog.status = dialogs::ExtractionStatus::Completed;
            } else {
                state.conversion_dialog.status = dialogs::ExtractionStatus::Failed;
            }
            // Auto-hide when completed unless minimized
            if !state.conversion_minimized {
                state.conversion_dialog.show = false;
            }
            state.conversion_child = None;
            state.conversion_rx = None;
            state.conversion_started = None;
        }
    }
}

/// Common archive file extensions
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz"];

/// Check if a filename has an archive extension
fn is_archive_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    ARCHIVE_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

/// Recursively search for a file by name in a directory
fn find_file_in_dir(dir: &std::path::Path, filename: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_file_in_dir(&path, filename) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(filename) {
                return Some(path);
            }
        }
    }
    None
}

/// Determine the best extraction strategy based on file type
fn determine_extraction_strategy(file_path: &str) -> arclain_core::OpenStrategy {
    let lower = file_path.to_lowercase();

    // Self-contained files: just extract the file itself
    let self_contained_extensions = [
        // Images
        ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".svg", ".ico", ".tiff", ".tif",
        // Documents
        ".pdf", ".txt", ".md", ".html", ".htm", ".xml", ".json", ".csv", // Audio
        ".mp3", ".wav", ".flac", ".ogg", ".m4a", ".aac", // Video
        ".mp4", ".mkv", ".avi", ".mov", ".webm", ".wmv",
    ];

    for ext in self_contained_extensions {
        if lower.ends_with(ext) {
            return arclain_core::OpenStrategy::FileOnly;
        }
    }

    // Executables and games: extract with dependencies
    let executable_extensions = [
        ".exe", ".dll", ".bat", ".cmd", ".ps1", // Windows
        ".sh", ".run", ".bin", // Linux
        ".app", // macOS
    ];

    for ext in executable_extensions {
        if lower.ends_with(ext) {
            return arclain_core::OpenStrategy::WithDependencies;
        }
    }

    // Default: same directory (covers most cases)
    arclain_core::OpenStrategy::SameDirectory
}

/// Open a file from the current archive by extracting to temp and launching
pub fn open_file_from_archive(
    state: &parking_lot::Mutex<crate::core::AppState>,
    file_path: &str,
    status_info: &mut crate::shared::components::StatusBarInfo,
) -> Option<std::path::PathBuf> {
    use arclain_core::FileOpener;

    let st = state.lock();
    let archive = match &st.current_archive {
        Some(a) => a.clone(),
        None => {
            status_info.message = "No archive open".to_string();
            return None;
        }
    };

    // Get all entry paths for dependency resolution
    let all_entries: Vec<String> = st.all_entries.iter().map(|e| e.path.clone()).collect();
    let backend = st.fallback_backend.clone();
    let password = st.current_password.clone();
    drop(st);

    // Create FileOpener
    let opener = match FileOpener::new() {
        Ok(o) => o,
        Err(e) => {
            status_info.message = format!("Failed to create temp directory: {}", e);
            return None;
        }
    };

    // Determine smart extraction strategy based on file type
    let strategy = determine_extraction_strategy(file_path);
    tracing::info!(
        "Using {:?} extraction strategy for: {}",
        strategy,
        file_path
    );

    // Determine files to extract
    let files_to_extract = opener.get_files_to_extract(file_path, &all_entries, strategy);

    tracing::info!(
        "Extracting {} files to temp for opening: {}",
        files_to_extract.len(),
        file_path
    );

    // Extract files
    let temp_dir = opener.temp_dir();
    let result = backend.extract_files(&archive, temp_dir, &files_to_extract, password.as_deref());

    if let Err(e) = result {
        status_info.message = format!("Failed to extract: {}", e);
        return None;
    }

    // Check if this is an archive file
    if is_archive_file(file_path) {
        // Return the extracted path so caller can open it as a new archive
        let extracted_path = temp_dir.join(file_path);
        if extracted_path.exists() {
            status_info.message = format!("Opening nested archive: {}", file_path);
            return Some(extracted_path);
        }
    }

    // Find the actual file in temp directory
    // 7z preserves full path structure, so file might be in subdirectory
    let file_full_path = temp_dir.join(file_path);

    // Prevent cleanup - we want to keep temp files for opening
    // Move this BEFORE any existence checks to prevent cleanup on early return
    let temp_dir_owned = temp_dir.to_path_buf();
    std::mem::forget(opener);

    // Debug: List what was actually extracted
    tracing::info!("Temp directory contents at: {}", temp_dir_owned.display());
    if let Ok(entries) = std::fs::read_dir(&temp_dir_owned) {
        for entry in entries.flatten() {
            tracing::info!("  -> {}", entry.path().display());
        }
    }

    let actual_file = if file_full_path.exists() {
        file_full_path
    } else {
        // Try to find by filename only (in case the path structure differs)
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        tracing::info!(
            "Searching for filename: {} in {}",
            filename,
            temp_dir_owned.display()
        );

        // Search for the file in temp directory
        find_file_in_dir(&temp_dir_owned, filename).unwrap_or(file_full_path)
    };

    tracing::info!("Attempting to open: {}", actual_file.display());

    if !actual_file.exists() {
        status_info.message = format!("File not found after extraction: {}", actual_file.display());
        return None;
    }

    // Open the file with system default using the full path
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        match Command::new("explorer").arg(&actual_file).spawn() {
            Ok(child) => {
                tracing::info!("Launched explorer with PID: {:?}", child.id());
            }
            Err(e) => {
                status_info.message = format!("Failed to open file: {}", e);
                return None;
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Command;
        if let Err(e) = Command::new("xdg-open").arg(&actual_file).spawn() {
            status_info.message = format!("Failed to open file: {}", e);
            return None;
        }
    }

    status_info.message = format!("Opened: {}", file_path);

    None
}
