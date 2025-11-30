use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use crate::{sevenzip::SevenZipCli, ArchiveBackend};

/// Generic game/product metadata from any source (DLSite, itch.io, Steam, etc.)
///
/// The `metadata_json` field contains a layered JSON structure:
/// ```json
/// {
///   "source": "dlsite",
///   "product_id": "RJ123456",
///   "common": {
///     "title": "Game Title",
///     "description": "...",
///     "tags": ["tag1", "tag2"],
///     "creator": "Creator Name",
///     "release_date": "2024-01-01"
///   },
///   "dlsite": {
///     // All DLSite-specific fields preserved here
///     "circle": "サークル名",
///     "work_format": "ゲーム",
///     // ... etc
///   }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct GameMetadata {
    /// Product ID - platform-specific identifier (e.g., "RJ123456", "itch-slug")
    pub product_id: String,

    /// Source platform (e.g., "dlsite", "itch", "steam")
    pub source: String,

    /// Title - extracted for convenience and folder naming
    pub title: String,

    /// Description - extracted for convenience
    pub description: Option<String>,

    /// Tags - extracted for convenience
    pub tags: Vec<String>,

    /// Release date - extracted for convenience
    pub release_date: Option<String>,

    /// Creator/Circle/Publisher - extracted for convenience
    pub creator: Option<String>,

    /// Screenshots to embed
    pub screenshots: Vec<ScreenshotData>,

    /// Full layered JSON with both common and platform-specific data
    /// This is what gets saved as metadata.json in the archive
    pub metadata_json: String,
}

/// Screenshot data provided by plugin
#[derive(Debug, Clone)]
pub enum ScreenshotData {
    FilePath(PathBuf), // Downloaded by plugin
    Base64(String),    // Base64-encoded
}

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
pub fn check_archive_structure(
    backend: &SevenZipCli,
    archive: &Path,
    expected: &ArchiveStructure,
) -> Result<bool> {
    info!(
        "Checking if archive {} follows structure for {}",
        archive.display(),
        expected.root_folder
    );

    // List archive contents
    let info = backend.list(archive, None)?;

    // Check for root folder
    let root_prefix = format!("{}/", expected.root_folder);
    let has_root = info
        .entries
        .iter()
        .any(|e| e.path.starts_with(&root_prefix));

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
pub fn needs_better_compression(backend: &SevenZipCli, archive: &Path) -> Result<bool> {
    info!("Checking compression settings: {}", archive.display());

    // Check if it's a 7z archive
    let kind = backend.identify(archive)?;
    if !matches!(kind, crate::ArchiveKind::SevenZ) {
        debug!("Archive is not 7z format, needs conversion");
        return Ok(true);
    }

    // For now, assume 7z archives may not use optimal compression
    // A more sophisticated check would parse the archive header
    debug!("Assuming 7z archive may need recompression for optimal settings");
    Ok(true)
}

/// Organize archive with game metadata (source-agnostic)
pub fn organize_archive(
    backend: &SevenZipCli,
    source: &Path,
    dest: &Path,
    metadata: &GameMetadata,
    temp_dir: &Path,
) -> Result<()> {
    info!(
        "Organizing archive {} with {} metadata for {}",
        source.display(),
        metadata.source,
        metadata.product_id
    );

    // Create unique temp directory
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let work_dir = temp_dir.join(format!("arclain_organize_{}", timestamp));
    let root_dir = work_dir.join(&metadata.product_id);
    std::fs::create_dir_all(&root_dir).context("creating temp dir")?;

    // RAII cleanup guard
    struct TempDirGuard {
        path: PathBuf,
    }
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            if let Err(e) = std::fs::remove_dir_all(&self.path) {
                error!("Failed to cleanup temp dir {}: {}", self.path.display(), e);
            }
        }
    }
    let _guard = TempDirGuard {
        path: work_dir.clone(),
    };

    // 1. Extract source to root_dir/game
    let game_dir = root_dir.join("game");
    std::fs::create_dir_all(&game_dir)?;

    debug!("Extracting source to game directory");
    backend
        .extract_all(source, &game_dir, None)
        .context("extracting source archive")?;

    // 2. Create metadata.json
    debug!("Creating metadata.json");
    let metadata_path = root_dir.join("metadata.json");
    std::fs::write(&metadata_path, &metadata.metadata_json)?;

    // 3. Create screenshots directory
    if !metadata.screenshots.is_empty() {
        debug!(
            "Creating screenshots directory with {} images",
            metadata.screenshots.len()
        );
        let screenshots_dir = root_dir.join("screenshots");
        std::fs::create_dir_all(&screenshots_dir)?;

        for (idx, screenshot) in metadata.screenshots.iter().enumerate() {
            let filename = format!("{:02}.jpg", idx + 1);
            let dest_path = screenshots_dir.join(&filename);

            match screenshot {
                ScreenshotData::FilePath(path) => {
                    debug!("Copying screenshot from {}", path.display());
                    std::fs::copy(path, &dest_path)?;
                }
                ScreenshotData::Base64(_data) => {
                    debug!("Decoding base64 screenshot {}", filename);
                    // TODO: Decode base64 and write
                    // For now, write placeholder or skip
                    info!("Base64 screenshots not yet implemented, skipping");
                }
            }
        }
    }

    // 4. Compress root_dir to dest
    debug!("Compressing organized structure to 7z");
    let dest_abs = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest)
    };

    // Use create_archive with 7z format
    backend
        .create_archive(&dest_abs, &[root_dir.clone()], "7z")
        .context("creating organized 7z archive")?;

    info!("Archive organization completed successfully");
    Ok(())
}
