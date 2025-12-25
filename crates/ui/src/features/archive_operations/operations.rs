use super::ArchiveOperationsState;
use crate::core::utils;
use crate::platform::{resume_process, suspend_process};
use crate::shared::dialogs;

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

    // Use the backend selector to get the proper backend for this archive type
    let backend = match st.backend_selector.select(&archive) {
        Ok(b) => b,
        Err(e) => {
            drop(st);
            status_info.message = format!("Failed to select backend: {}", e);
            return None;
        }
    };

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

    // Get temp directory for extraction
    let temp_dir = opener.temp_dir().to_path_buf();
    let temp_dir_for_thread = temp_dir.clone();

    // Get signals from state for progress updates
    let st = state.lock();
    let signals = st.signals.clone();
    drop(st);

    // Reset cancellation token
    signals
        .extraction_cancel
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Set initial extraction progress
    let file_path_str = file_path.to_string();
    let total_files = files_to_extract.len();
    signals
        .extraction_progress
        .set(Some(crate::core::signals::ExtractionProgressState {
            current_file: "Starting extraction...".to_string(),
            percent: 0,
            current: 0,
            total: total_files,
            complete: false,
            error: None,
            file_to_open: None,
            cancelled: false,
        }));

    // Clone data for thread
    let archive_clone = archive.clone();
    let files_clone = files_to_extract.clone();
    let password_clone = password.clone();
    let backend_clone = backend.clone();
    let signals_clone = signals.clone();
    let file_path_clone = file_path_str.clone();
    let cancel_token = signals.extraction_cancel.clone();

    // Run extraction in background thread
    std::thread::spawn(move || {
        // Clone again for the progress callback
        let signals_for_callback = signals_clone.clone();

        let result = backend_clone.extract_files_with_progress(
            &archive_clone,
            &temp_dir_for_thread,
            &files_clone,
            password_clone.as_deref(),
            Some(&move |progress: arclain_core::ExtractionProgress| {
                tracing::debug!(
                    "Progress callback: {}/{} ({}%) - {}",
                    progress.current,
                    progress.total,
                    progress.percent,
                    progress.current_file
                );
                signals_for_callback.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: progress.current_file.clone(),
                        percent: progress.percent,
                        current: progress.current,
                        total: progress.total,
                        complete: false,
                        error: None,
                        file_to_open: None,
                        cancelled: false,
                    },
                ));
            }),
            Some(&cancel_token), // Pass cancellation token
        );

        match result {
            Ok(_) => {
                // Find the file and signal completion
                let file_full_path = temp_dir_for_thread.join(&file_path_clone);
                let actual_file = if file_full_path.exists() {
                    file_full_path
                } else {
                    // Search for file by name
                    find_file_in_dir_static(&temp_dir_for_thread, &file_path_clone)
                        .unwrap_or(file_full_path)
                };

                signals_clone.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: "Extraction complete".to_string(),
                        percent: 100,
                        current: total_files,
                        total: total_files,
                        complete: true,
                        error: None,
                        file_to_open: Some(actual_file),
                        cancelled: false,
                    },
                ));
            }
            Err(e) => {
                signals_clone.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: "Extraction failed".to_string(),
                        percent: 0,
                        current: 0,
                        total: total_files,
                        complete: true,
                        error: Some(format!("{}", e)),
                        file_to_open: None,
                        cancelled: false,
                    },
                ));
            }
        }
    });

    // Prevent cleanup - we want to keep temp files for opening
    std::mem::forget(opener);

    // Return None - the file will be opened when extraction signal completes
    // The UI will handle detecting complete=true and opening the file
    status_info.message = format!("Extracting {} files...", total_files);
    None
}

/// Static helper to find file in directory (for use in thread)
fn find_file_in_dir_static(dir: &std::path::Path, filename: &str) -> Option<std::path::PathBuf> {
    // Get just the filename without path
    let target = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(filename);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_file_in_dir_static(&path, target) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(path);
            }
        }
    }
    None
}
