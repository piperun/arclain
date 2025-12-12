use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use crate::Archive;

// Re-export items to maintain API compatibility
pub use super::checks::{
    check_archive_structure, needs_better_compression, verify_archive_encryption, ArchiveStructure,
};
pub use super::flatten::find_and_flatten_game_content; // Kept as pub if needed by engine?
pub use super::metadata::{GameMetadata, ScreenshotData};
pub use super::tasks::*;

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
    plan: &crate::features::organization::engine::OrganizationPlan,
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

    // RAII cleanup guard - local helper here or reuse from tasks?
    // We can reuse TempDirGuard but construction is slightly different (we built the path manually here)
    // So we'll just implement the drop guard locally as before or use the public one if we adapt.
    // Let's stick to the inline guard for now or construct the public one.
    // The public TempDirGuard fields are private (default Rust struct visibility).
    // We should probably rely on `create_work_directory` but the naming scheme here is specific ("arc_...").
    // Let's just reimplement the simple guard here.
    struct LocalTempDirGuard {
        path: PathBuf,
    }
    impl Drop for LocalTempDirGuard {
        fn drop(&mut self) {
            if let Err(e) = std::fs::remove_dir_all(&self.path) {
                error!("Failed to cleanup temp dir {}: {}", self.path.display(), e);
            }
        }
    }
    let _guard = LocalTempDirGuard {
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
        super::flatten::find_and_flatten_game_content(&source_extracted, &game_dir)?;
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
