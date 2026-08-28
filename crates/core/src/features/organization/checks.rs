use crate::Archive;
use anyhow::Result;
use tracing::{debug, info};

/// Expected archive structure
#[derive(Debug, Clone)]
pub struct ArchiveStructure {
    pub root_folder: String,
    pub has_game_folder: bool,
    pub has_metadata: bool,
    pub has_screenshots: bool,
    pub uses_optimal_compression: bool,
}

/// Check if archive already follows the desired structure
pub fn check_archive_structure(archive: &Archive, expected: &ArchiveStructure) -> Result<bool> {
    info!(
        "Checking if archive {} follows structure for {}",
        archive.path().display(),
        expected.root_folder
    );

    // List archive contents
    let info = archive
        .backend()
        .list(archive.path(), archive.password_ref())?;

    // Check for root folder
    let root_prefix = format!("{}/", expected.root_folder);
    let has_root = info
        .entries
        .iter()
        .any(|e| e.path.starts_with(&root_prefix) || e.path == expected.root_folder);

    // Note: Added explicit check for root folder match itself just in case

    if !has_root {
        debug!("Archive missing root folder: {}", expected.root_folder);
        return Ok(false);
    }

    // Check for game folder
    let game_prefix = format!("{}/game/", expected.root_folder);
    let has_game = info
        .entries
        .iter()
        .any(|e| e.path.starts_with(&game_prefix));

    if expected.has_game_folder && !has_game {
        debug!("Archive missing game folder");
        return Ok(false);
    }

    // Check for metadata.json
    let metadata_path = format!("{}/metadata.json", expected.root_folder);
    let has_metadata = info.entries.iter().any(|e| e.path == metadata_path);

    if expected.has_metadata && !has_metadata {
        debug!("Archive missing metadata.json");
        return Ok(false);
    }

    // Check for screenshots folder
    let screenshots_prefix = format!("{}/screenshots/", expected.root_folder);
    let has_screenshots = info
        .entries
        .iter()
        .any(|e| e.path.starts_with(&screenshots_prefix));

    if expected.has_screenshots && !has_screenshots {
        debug!("Archive missing screenshots folder");
        return Ok(false);
    }

    info!("Archive structure matches expected format");
    Ok(true)
}

/// Check if archive uses optimal compression
pub fn needs_better_compression(archive: &Archive) -> Result<bool> {
    info!(
        "Checking compression settings: {}",
        archive.path().display()
    );

    // Check if it's a 7z archive
    let kind = archive.backend().identify(archive.path())?;
    if !matches!(kind, crate::ArchiveKind::SevenZ) {
        debug!("Archive is not 7z format, needs conversion");
        return Ok(true);
    }

    // For now, assume 7z archives may not use optimal compression
    // A more sophisticated check would parse the archive header
    debug!("Assuming 7z archive may need recompression for optimal settings");
    Ok(true)
}
