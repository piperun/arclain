use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info, warn};

/// Common archive file extensions
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "tar.gz", "tar.bz2", "tar.xz",
];

/// Check if a file path has an archive extension
pub fn is_archive_extension(path: &Path) -> bool {
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Check compound extensions first
    for ext in ARCHIVE_EXTENSIONS {
        if filename.ends_with(&format!(".{}", ext)) {
            return true;
        }
    }
    false
}

/// Find all archive files in a directory
pub fn find_nested_archives(dir: &Path) -> Result<Vec<(std::path::PathBuf, u64)>> {
    let mut archives = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && is_archive_extension(&path) {
            let size = entry.metadata()?.len();
            archives.push((path, size));
        }
    }

    // Sort by size descending (largest first)
    archives.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(archives)
}

/// Find the actual game content folder and flatten to Game/ directory
///
/// This recursively searches for game content indicators (exe files, package.json, index.html, etc.)
/// and moves that content to the destination, removing all wrapper directories.
///
/// If no game content is found but nested archives exist, it will extract the largest one
/// and recursively search within it.
pub fn find_and_flatten_game_content(source: &Path, dest: &Path) -> Result<()> {
    find_and_flatten_game_content_with_depth(source, dest, 0)
}

/// Internal implementation with depth tracking to prevent infinite recursion
fn find_and_flatten_game_content_with_depth(
    source: &Path,
    dest: &Path,
    depth: usize,
) -> Result<()> {
    const MAX_NESTED_DEPTH: usize = 3;

    if depth > MAX_NESTED_DEPTH {
        return Err(anyhow::anyhow!(
            "Maximum nested archive depth ({}) exceeded",
            MAX_NESTED_DEPTH
        ));
    }

    debug!(
        "Searching for game content in: {} (depth: {})",
        source.display(),
        depth
    );

    // Game content indicators
    let game_indicators = [
        "Game.exe",
        "game.exe",
        "nw.exe",
        "index.html",
        "package.json",
        "www",  // RPG Maker folder
        "data", // Common game data folder
        "js",   // JavaScript games
    ];

    // Check if current directory IS the game content folder
    let entries: Vec<_> = std::fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;

    // Count how many indicators we find
    let mut indicator_count = 0;
    let mut has_any_exe = false;

    for entry in &entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Check for standard indicators
        for indicator in &game_indicators {
            if name_str.eq_ignore_ascii_case(indicator) {
                indicator_count += 1;
                debug!("Found game indicator: {}", name_str);
                break;
            }
        }

        // Check for any .exe file (flexible indicator for custom-named executables)
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && name_str.to_lowercase().ends_with(".exe")
        {
            has_any_exe = true;
        }
    }

    // Any .exe file counts as an indicator (catches girlgame_600x900w.exe, etc.)
    // An exe file alone is sufficient - many games only have exe + data files
    if has_any_exe {
        indicator_count += 1;
        debug!("Found .exe file indicator");
    }

    // If we found game indicators OR have an exe file, this IS the game folder
    // Single .exe is sufficient since that's a strong signal of game content
    let is_game_folder = indicator_count >= 2 || has_any_exe;

    if is_game_folder {
        info!(
            "Found game content folder with {} indicators, flattening {} files/dirs",
            indicator_count,
            entries.len()
        );

        // Move all contents from source to dest
        for entry in entries {
            let src_path = entry.path();
            let dest_path = dest.join(entry.file_name());

            debug!("Moving {} -> {}", src_path.display(), dest_path.display());
            std::fs::rename(&src_path, &dest_path).or_else(|_| {
                // If rename fails (cross-device), copy recursively
                if src_path.is_dir() {
                    copy_dir_recursive(&src_path, &dest_path)
                } else {
                    std::fs::copy(&src_path, &dest_path)
                        .map(|_| ())
                        .context("copying file")
                }
            })?;
        }

        return Ok(());
    }

    // Otherwise, recursively search subdirectories
    debug!(
        "Not game folder (only {} indicators), searching {} subdirectories",
        indicator_count,
        entries
            .iter()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .count()
    );

    for entry in &entries {
        if entry.file_type()?.is_dir() {
            let subdir = entry.path();
            debug!("Checking subdirectory: {}", subdir.display());

            // Try to find game content in this subdirectory
            match find_and_flatten_game_content_with_depth(&subdir, dest, depth) {
                Ok(_) => return Ok(()), // Found it!
                Err(_) => continue,     // Not in this subdir, try next
            }
        }
    }

    // If we get here, we didn't find game content indicators
    // Check for nested archives and try extracting the largest one
    if depth < MAX_NESTED_DEPTH {
        let nested_archives = find_nested_archives(source)?;

        if !nested_archives.is_empty() {
            let (largest_archive, size) = &nested_archives[0];
            info!(
                "No game content found. Trying nested archive: {} ({} bytes)",
                largest_archive.display(),
                size
            );

            // Create temp directory for extraction
            let temp_extract_dir = source.join(".arclain_nested_extract");
            std::fs::create_dir_all(&temp_extract_dir)?;

            // Try to extract using backend selector
            match extract_nested_archive(largest_archive, &temp_extract_dir) {
                Ok(_) => {
                    // Recursively search the extracted content
                    match find_and_flatten_game_content_with_depth(
                        &temp_extract_dir,
                        dest,
                        depth + 1,
                    ) {
                        Ok(_) => {
                            // Cleanup temp dir
                            let _ = std::fs::remove_dir_all(&temp_extract_dir);
                            return Ok(());
                        }
                        Err(e) => {
                            let _ = std::fs::remove_dir_all(&temp_extract_dir);
                            warn!("Nested archive didn't contain game content: {}", e);
                        }
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&temp_extract_dir);
                    warn!("Failed to extract nested archive: {}", e);
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "Could not find game content folder in extracted archive"
    ))
}

/// Extract a nested archive using the 7z CLI backend
fn extract_nested_archive(archive: &Path, dest: &Path) -> Result<()> {
    use crate::backends::sevenz_cli::SevenZipCli;
    use crate::ArchiveBackend;

    let backend = SevenZipCli::detect(None)?;
    backend.extract_all(archive, dest, None)
}

/// Recursively copy a directory
pub fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            std::fs::copy(&src_path, &dest_path)?;
        }
    }

    Ok(())
}
