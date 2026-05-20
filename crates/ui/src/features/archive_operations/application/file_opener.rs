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

/// Open a file from the current archive by extracting to temp and launching
pub fn open_file_from_archive(
    state: &parking_lot::Mutex<crate::core::AppState>,
    file_path: &str,
    status_info: &mut crate::shared::components::StatusBarInfo,
) -> Option<std::path::PathBuf> {
    use arclain_core::FileOpener;

    let st = state.lock();
    let signals = st.signals.clone();
    let tab = signals.tabs.get().active().clone();

    let archive = match tab.archive_path.get() {
        Some(a) => a,
        None => {
            drop(st);
            status_info.message = "No archive open".to_string();
            return None;
        }
    };

    // Get all entry paths for dependency resolution
    let entries_arc = tab.entries.get();
    let all_entries: Vec<String> = entries_arc.iter().map(|e| e.path.clone()).collect();

    // Use the backend selector to get the proper backend for this archive type
    // Backend selector is still on AppState
    let backend = match st.backend_selector.select(&archive) {
        Ok(b) => b,
        Err(e) => {
            drop(st);
            status_info.message = format!("Failed to select backend: {}", e);
            return None;
        }
    };

    let password = tab.current_password.get();
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
            requested_file_path: Some(file_path_str.clone()),
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
        let file_path_for_callback = file_path_clone.clone();

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
                        requested_file_path: Some(file_path_for_callback.clone()),
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
                        requested_file_path: Some(file_path_clone.clone()),
                    },
                ));
            }
            Err(e) => {
                // Use `{:#}` to flatten the anyhow context chain into
                // the error string. The outer wrappers (e.g. the
                // fallback_backend's "Both backends failed to extract
                // files") would otherwise hide the underlying
                // "Incorrect password" / "Wrong password" / etc. that
                // process_extraction_progress needs to see to route
                // the password dialog.
                signals_clone.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: "Extraction failed".to_string(),
                        percent: 0,
                        current: 0,
                        total: total_files,
                        complete: true,
                        error: Some(format!("{:#}", e)),
                        file_to_open: None,
                        requested_file_path: Some(file_path_clone.clone()),
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
