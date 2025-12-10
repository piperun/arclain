use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use crate::Archive;

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    #[serde(skip)]
    pub metadata_json: String,
}

impl GameMetadata {
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        let mut metadata: Self = serde_json::from_str(json)?;
        metadata.metadata_json = json.to_string();

        // DEBUG: Log what we parsed
        tracing::info!(
            "[GameMetadata] Parsed from JSON - title: {:?}, creator: {:?}",
            metadata.title,
            metadata.creator
        );

        // If creator is None, try to extract from 'circle' field in the raw JSON
        if metadata.creator.is_none() {
            if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(json) {
                if let Some(circle) = json_value.get("circle").and_then(|v| v.as_str()) {
                    tracing::info!(
                        "[GameMetadata] Found 'circle' field in JSON: {}, using as creator",
                        circle
                    );
                    metadata.creator = Some(circle.to_string());
                }
            }
        }

        Ok(metadata)
    }
}

/// Screenshot data provided by plugin
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// RAII guard for temporary directory cleanup
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

/// Create a unique temporary work directory
fn create_work_directory(temp_dir: &Path, prefix: &str) -> Result<(PathBuf, TempDirGuard)> {
    debug!(
        "Creating work directory with prefix '{}' in {:?}",
        prefix, temp_dir
    );

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let work_dir = temp_dir.join(format!("{}_{}", prefix, timestamp));
    std::fs::create_dir_all(&work_dir).context("creating work directory")?;

    info!("Created work directory: {:?}", work_dir);

    let guard = TempDirGuard {
        path: work_dir.clone(),
    };

    Ok((work_dir, guard))
}

/// Verify archive encryption status and password availability
fn verify_archive_encryption(archive: &Archive) -> Result<()> {
    info!(
        "Verifying archive encryption status for: {}",
        archive.path().display()
    );

    let archive_info = archive
        .backend()
        .list(archive.path(), archive.password_ref())
        .context("listing archive contents")?;

    debug!(
        "Archive encryption status: encrypted={}, headers_encrypted={}, method={:?}",
        archive_info.encrypted, archive_info.headers_encrypted, archive_info.encryption_method
    );

    if archive_info.encrypted && !archive.has_password() {
        error!(
            "Archive is encrypted but no password provided: {}",
            archive.path().display()
        );
        return Err(anyhow::anyhow!(
            "Archive '{}' contains encrypted files but no password was provided.",
            archive.path().display()
        ));
    }

    if archive_info.encrypted {
        info!("Archive is encrypted and password is available");
    } else {
        info!("Archive is not encrypted");
    }

    Ok(())
}

/// Extract archive contents to a temporary directory
fn extract_archive_to_temp(archive: &Archive, work_dir: &Path) -> Result<PathBuf> {
    info!("Extracting archive: {}", archive.path().display());

    let extract_temp = work_dir.join("extract_temp");
    std::fs::create_dir_all(&extract_temp)?;

    debug!("Extract destination: {:?}", extract_temp);
    archive
        .extract_all(&extract_temp)
        .context("extracting source archive")?;

    info!("Archive extraction completed successfully");
    Ok(extract_temp)
}

/// Organize game content by finding and flattening to Game/ directory
fn organize_game_content(extract_temp: &Path, root_dir: &Path) -> Result<PathBuf> {
    info!("Organizing game content from extracted files");

    let game_dir = root_dir.join("Game");
    std::fs::create_dir_all(&game_dir)?;

    debug!("Game directory destination: {:?}", game_dir);
    find_and_flatten_game_content(extract_temp, &game_dir)?;

    info!("Game content organization completed");
    Ok(game_dir)
}

/// Create metadata.json file in the root directory
fn create_metadata_file(root_dir: &Path, metadata: &GameMetadata) -> Result<()> {
    info!(
        "Creating metadata.json for product: {}",
        metadata.product_id
    );

    let metadata_path = root_dir.join("metadata.json");
    debug!("Metadata file path: {:?}", metadata_path);

    std::fs::write(&metadata_path, &metadata.metadata_json).context("writing metadata.json")?;

    info!(
        "Metadata file created successfully ({} bytes)",
        metadata.metadata_json.len()
    );
    Ok(())
}

/// Process and save screenshot to destination
fn process_screenshot(screenshot: &ScreenshotData, dest_path: &Path, filename: &str) -> Result<()> {
    match screenshot {
        ScreenshotData::FilePath(path) => {
            debug!("Copying screenshot '{}' from {}", filename, path.display());
            let bytes_copied = std::fs::copy(path, dest_path).context("copying screenshot file")?;
            debug!("Screenshot copied: {} bytes", bytes_copied);
        }
        ScreenshotData::Base64(data) => {
            debug!(
                "Decoding base64 screenshot '{}' ({} bytes)",
                filename,
                data.len()
            );
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .context("decoding base64 screenshot")?;
            std::fs::write(dest_path, &bytes).context("writing decoded screenshot")?;
            debug!("Base64 screenshot decoded and saved: {} bytes", bytes.len());
        }
    }
    Ok(())
}

/// Create screenshots directory and process all screenshots
fn create_screenshots_directory(root_dir: &Path, metadata: &GameMetadata) -> Result<()> {
    info!("Setting up screenshots directory");

    let screenshots_dir = root_dir.join("screenshots");
    std::fs::create_dir_all(&screenshots_dir)?;
    debug!("Screenshots directory created: {:?}", screenshots_dir);

    if metadata.screenshots.is_empty() {
        info!("No screenshots provided in metadata - directory created but empty");
        return Ok(());
    }

    info!("Processing {} screenshots", metadata.screenshots.len());

    for (idx, screenshot) in metadata.screenshots.iter().enumerate() {
        let filename = format!("{:02}.jpg", idx + 1);
        let dest_path = screenshots_dir.join(&filename);

        process_screenshot(screenshot, &dest_path, &filename)
            .with_context(|| format!("processing screenshot {}", filename))?;
    }

    info!("All screenshots processed successfully");
    Ok(())
}

/// Create the final 7z archive from organized content
fn create_final_archive(archive: &Archive, dest: &Path, root_dir: &Path) -> Result<()> {
    info!("Creating final 7z archive");
    debug!("Source directory: {:?}", root_dir);
    debug!("Destination path: {:?}", dest);

    let dest_abs = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest)
    };

    debug!("Absolute destination path: {:?}", dest_abs);

    info!("Compressing organized structure to 7z format");
    archive
        .backend()
        .create_archive(&dest_abs, &[root_dir.to_path_buf()], "7z")
        .context("creating organized 7z archive")?;

    info!("Final archive created successfully: {}", dest_abs.display());
    Ok(())
}

/// Organize archive with game metadata using Archive (dependency injection pattern)
///
/// This is the primary API for organizing archives. Pass in an Archive handle
/// which encapsulates the backend, file path, and any necessary credentials.
pub fn organize_archive(
    archive: &Archive,
    dest: &Path,
    metadata: &GameMetadata,
    temp_dir: &Path,
) -> Result<()> {
    info!(
        "Organizing archive {} with {} metadata for {}",
        archive.path().display(),
        metadata.source,
        metadata.product_id
    );

    // Step 1: Create work directory with cleanup guard
    let (work_dir, _guard) = create_work_directory(temp_dir, "arclain_organize")?;
    let root_dir = work_dir.join(&metadata.product_id);
    std::fs::create_dir_all(&root_dir).context("creating root dir")?;

    // Step 2: Verify encryption status
    verify_archive_encryption(archive)?;

    // Step 3: Extract archive
    let extract_temp = extract_archive_to_temp(archive, &work_dir)?;

    // Step 4: Organize game content
    organize_game_content(&extract_temp, &root_dir)?;

    // Step 5: Create metadata file
    create_metadata_file(&root_dir, metadata)?;

    // Step 6: Process screenshots
    create_screenshots_directory(&root_dir, metadata)?;

    // Step 7: Create final archive
    create_final_archive(archive, dest, &root_dir)?;

    info!("Archive organization completed successfully");
    Ok(())
}

/// Execute a generic organization plan
pub fn execute_organization_plan(
    archive: &Archive,
    dest: &Path,
    plan: &crate::organization::engine::OrganizationPlan,
    temp_dir: &Path,
) -> Result<()> {
    info!(
        "Executing organization plan '{}' for archive {}",
        plan.rule_name,
        archive.path().display()
    );

    // Create unique temp directory with short, readable name
    // Format: arc_<secs>_<pid> (saves ~25 chars vs old format for Windows path limits)
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let work_dir = temp_dir.join(format!("arc_{}_{}", secs, std::process::id()));
    let source_extracted = work_dir.join("src");
    let organized_dir = work_dir.join("out");

    std::fs::create_dir_all(&source_extracted).context("creating temp source dir")?;
    std::fs::create_dir_all(&organized_dir).context("creating temp organized dir")?;

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

    // 1. Extract source
    debug!("Extracting source archive");
    archive
        .extract_all(&source_extracted)
        .context("extracting source archive")?;

    // 2. Move files according to plan
    debug!("Moving files according to plan");

    if plan.use_standard_layout {
        // Standard Layout: Smart Flattening
        // We ignore explicit moves for game content and use the flattener
        let root_folder = organized_dir.join(&plan.root_folder);
        let game_dir = root_folder.join("Game");
        std::fs::create_dir_all(&game_dir)?;

        debug!(
            "Using Standard Layout - Flattening game content to {:?}",
            game_dir
        );
        find_and_flatten_game_content(&source_extracted, &game_dir)?;
    } else {
        // Legacy/Custom Layout: Explicit moves
        for (src_rel, dst_rel) in &plan.moves {
            let src_path = source_extracted.join(src_rel);
            let dst_path = organized_dir.join(dst_rel);

            if src_path.exists() {
                if let Some(parent) = dst_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                // Use copy instead of rename to avoid issues if we want to keep source for some reason
                // (though we delete it later). Rename is faster but cross-device issues might occur if temp is weird.
                // Since it's all in temp, rename should be fine.
                std::fs::rename(&src_path, &dst_path)
                    .or_else(|_| std::fs::copy(&src_path, &dst_path).map(|_| ()))?;
            } else {
                debug!("Source file not found (maybe directory?): {}", src_rel);
            }
        }
    }

    // 2b. Write generated files (e.g. metadata.json)
    debug!("Writing generated files");
    for (rel_path, content) in &plan.generated_files {
        let dst_path = organized_dir.join(rel_path);
        if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dst_path, content)?;
    }

    // 2c. Download files (e.g. screenshots)
    if !plan.downloads.is_empty() {
        debug!("Downloading {} files", plan.downloads.len());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Arclain/1.0")
            .build()?;

        for download in &plan.downloads {
            let dst_path = organized_dir.join(&download.dest_path);
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            debug!("Downloading {} to {}", download.url, download.dest_path);
            match client.get(&download.url).send() {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes() {
                            if let Err(e) = std::fs::write(&dst_path, bytes) {
                                error!(
                                    "Failed to write downloaded file {}: {}",
                                    download.dest_path, e
                                );
                            }
                        } else {
                            error!("Failed to get bytes for {}", download.url);
                        }
                    } else {
                        error!(
                            "Failed to download {}: status {}",
                            download.url,
                            resp.status()
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to download {}: {}", download.url, e);
                }
            }
        }
    }

    // 3. Compress organized directory to dest
    debug!("Compressing organized structure to 7z");
    let dest_abs = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest)
    };

    // We want to compress the CONTENTS of organized_dir, but create_archive takes a list of files/folders.
    // If we pass [organized_dir], it will create a root folder named "organized" inside the archive?
    // No, create_archive usually takes the items to put at root.
    // But our plan includes the root folder in the dst_rel (e.g. "Game/file.txt").
    // So organized_dir contains "Game" folder.
    // So we should pass [organized_dir/Game] (or whatever is inside organized_dir).

    // Actually, if organized_dir contains "Game", and we want the archive to contain "Game",
    // we should pass `organized_dir/Game` to create_archive?
    // Wait, create_archive takes `paths: &[PathBuf]`.
    // If I pass `path/to/Game`, 7z usually stores "Game/...".
    // So I should list the children of organized_dir and pass them.

    let mut items_to_compress = Vec::new();
    for entry in std::fs::read_dir(&organized_dir)? {
        let entry = entry?;
        items_to_compress.push(entry.path());
    }

    if items_to_compress.is_empty() {
        return Err(anyhow::anyhow!(
            "Organized directory is empty, nothing to compress"
        ));
    }

    // Use the backend from the archive handle to create the new archive
    archive
        .backend()
        .create_archive(&dest_abs, &items_to_compress, "7z")
        .context("creating organized 7z archive")?;

    info!("Plan execution completed successfully");
    Ok(())
}

/// Find the actual game content folder and flatten it to the destination
///
/// This recursively searches for game content indicators (exe files, package.json, index.html, etc.)
/// and moves that content to the destination, removing all wrapper directories.
fn find_and_flatten_game_content(source: &Path, dest: &Path) -> Result<()> {
    debug!("Searching for game content in: {}", source.display());

    // Game content indicators - files that indicate we've found the actual game folder
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
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
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
