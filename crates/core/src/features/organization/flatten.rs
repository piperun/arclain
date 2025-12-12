use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

/// Find the actual game content folder and flatten to Game/ directory
///
/// This recursively searches for game content indicators (exe files, package.json, index.html, etc.)
/// and moves that content to the destination, removing all wrapper directories.
pub fn find_and_flatten_game_content(source: &Path, dest: &Path) -> Result<()> {
    debug!("Searching for game content in: {}", source.display());

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
    for entry in &entries {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        for indicator in &game_indicators {
            if name_str.eq_ignore_ascii_case(indicator) {
                indicator_count += 1;
                debug!("Found game indicator: {}", name_str);
                break;
            }
        }
    }

    // If we found 2+ indicators, this IS the game folder
    if indicator_count >= 2 {
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

    for entry in entries {
        if entry.file_type()?.is_dir() {
            let subdir = entry.path();
            debug!("Checking subdirectory: {}", subdir.display());

            // Try to find game content in this subdirectory
            match find_and_flatten_game_content(&subdir, dest) {
                Ok(_) => return Ok(()), // Found it!
                Err(_) => continue,     // Not in this subdir, try next
            }
        }
    }

    // If we get here, we didn't find game content indicators
    Err(anyhow::anyhow!(
        "Could not find game content folder in extracted archive"
    ))
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
