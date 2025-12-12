use anyhow::{Context, Result};
use base64::Engine;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use super::metadata::{GameMetadata, ScreenshotData};
use crate::Archive;

/// RAII guard for temporary directory cleanup
pub struct TempDirGuard {
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
pub fn create_work_directory(temp_dir: &Path, prefix: &str) -> Result<(PathBuf, TempDirGuard)> {
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

/// Extract archive contents to a temporary directory
pub fn extract_archive_to_temp(archive: &Archive, work_dir: &Path) -> Result<PathBuf> {
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
pub fn organize_game_content(extract_temp: &Path, root_dir: &Path) -> Result<PathBuf> {
    info!("Organizing game content from extracted files");

    let game_dir = root_dir.join("Game");
    std::fs::create_dir_all(&game_dir)?;

    debug!("Game directory destination: {:?}", game_dir);
    super::flatten::find_and_flatten_game_content(extract_temp, &game_dir)?;

    info!("Game content organization completed");
    Ok(game_dir)
}

/// Create metadata.json file in the root directory
pub fn create_metadata_file(root_dir: &Path, metadata: &GameMetadata) -> Result<()> {
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
pub fn process_screenshot(
    screenshot: &ScreenshotData,
    dest_path: &Path,
    filename: &str,
) -> Result<()> {
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
pub fn create_screenshots_directory(root_dir: &Path, metadata: &GameMetadata) -> Result<()> {
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
pub fn create_final_archive(archive: &Archive, dest: &Path, root_dir: &Path) -> Result<()> {
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
