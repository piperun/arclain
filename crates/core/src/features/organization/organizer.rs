use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use crate::utilities::CheckedRelativePath;
use crate::Archive;

// Re-export items to maintain API compatibility
pub use super::checks::{
    check_archive_structure, needs_better_compression, verify_archive_encryption, ArchiveStructure,
};
pub use super::flatten::find_and_flatten_game_content; // Kept as pub if needed by engine?
pub use super::metadata::{GameMetadata, ScreenshotData};
pub use super::tasks::*;

pub(crate) fn open_plan_metadata_handle(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

        return std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path);
    }

    #[cfg(not(windows))]
    std::fs::File::open(path)
}

#[derive(Debug, Clone)]
struct PreservedPlanMetadata {
    permissions: std::fs::Permissions,
    accessed: std::time::SystemTime,
    modified: std::time::SystemTime,
    #[cfg(any(windows, target_os = "macos"))]
    created: std::time::SystemTime,
}

impl PreservedPlanMetadata {
    fn capture(metadata: &std::fs::Metadata, source: &Path) -> Result<Self> {
        Ok(Self {
            permissions: metadata.permissions(),
            accessed: metadata
                .accessed()
                .with_context(|| format!("reading access time for {}", source.display()))?,
            modified: metadata
                .modified()
                .with_context(|| format!("reading modification time for {}", source.display()))?,
            #[cfg(any(windows, target_os = "macos"))]
            created: metadata
                .created()
                .with_context(|| format!("reading creation time for {}", source.display()))?,
        })
    }

    fn apply_to_file(&self, file: &std::fs::File, destination: &Path) -> Result<()> {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::FileTimesExt;
        #[cfg(windows)]
        use std::os::windows::fs::FileTimesExt;

        let times = std::fs::FileTimes::new()
            .set_accessed(self.accessed)
            .set_modified(self.modified);
        #[cfg(any(windows, target_os = "macos"))]
        let times = times.set_created(self.created);

        file.set_times(times)
            .with_context(|| format!("preserving timestamps on {}", destination.display()))?;
        file.set_permissions(self.permissions.clone())
            .with_context(|| format!("preserving permissions on {}", destination.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeferredPlanMetadata {
    destination: CheckedRelativePath,
    metadata: PreservedPlanMetadata,
    is_directory: bool,
}

/// Persist a plan-controlled file without following or replacing an existing
/// final leaf. The temporary file is created beside the destination so the
/// final no-clobber operation is atomic on the destination filesystem.
pub(crate) fn persist_plan_output<R: std::io::Read + ?Sized>(
    root: &Path,
    relative: &CheckedRelativePath,
    reader: &mut R,
) -> Result<()> {
    persist_plan_output_with_metadata(root, relative, reader, None)
}

fn persist_plan_output_with_metadata<R: std::io::Read + ?Sized>(
    root: &Path,
    relative: &CheckedRelativePath,
    reader: &mut R,
    metadata: Option<&PreservedPlanMetadata>,
) -> Result<()> {
    let output = relative.resolve_under(root)?;
    let parent = output
        .parent()
        .context("organization output has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating organization output parent {}", parent.display()))?;

    let checked_output = relative.resolve_under(root)?;
    let checked_parent = checked_output
        .parent()
        .context("organization output has no checked parent")?;
    let mut staged = tempfile::NamedTempFile::new_in(checked_parent).with_context(|| {
        format!(
            "staging organization output in {}",
            checked_parent.display()
        )
    })?;
    std::io::copy(reader, staged.as_file_mut())
        .with_context(|| format!("writing staged output {}", checked_output.display()))?;
    staged
        .flush()
        .with_context(|| format!("flushing staged output {}", checked_output.display()))?;
    if let Some(metadata) = metadata {
        metadata.apply_to_file(staged.as_file(), &checked_output)?;
    }
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("syncing staged output {}", checked_output.display()))?;

    let checked_output = relative.resolve_under(root)?;
    let persisted = staged
        .persist_noclobber(&checked_output)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "persisting organization output {}",
                checked_output.display()
            )
        })?;
    // tempfile clears Windows temporary-file attributes during persistence,
    // which also clears read-only. Reapply after the no-clobber move while the
    // returned handle still identifies the persisted file.
    if let Some(metadata) = metadata {
        metadata.apply_to_file(&persisted, &checked_output)?;
        persisted
            .sync_all()
            .with_context(|| format!("syncing persisted output {}", checked_output.display()))?;
    }
    Ok(())
}

/// Copy a plan source into an owned output tree without replacing existing
/// content. Directories are merged recursively so ancestor/descendant plan
/// destinations are supported, while duplicate leaves fail closed.
pub(crate) fn copy_plan_source(
    output_root: &Path,
    destination: &CheckedRelativePath,
    source: &Path,
    deferred_metadata: &mut Vec<DeferredPlanMetadata>,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("inspecting organization source {}", source.display()))?;
    let preserved_metadata = PreservedPlanMetadata::capture(&metadata, source)?;

    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "organization source may not be a symlink: {}",
            source.display()
        );
    }

    if metadata.is_file() {
        let mut source_file = std::fs::File::open(source)
            .with_context(|| format!("opening organization source {}", source.display()))?;
        persist_plan_output_with_metadata(
            output_root,
            destination,
            &mut source_file,
            Some(&preserved_metadata),
        )
        .with_context(|| format!("copying organization source {}", source.display()))?;
        deferred_metadata.push(DeferredPlanMetadata {
            destination: destination.clone(),
            metadata: preserved_metadata,
            is_directory: false,
        });
        return Ok(());
    }

    if !metadata.is_dir() {
        anyhow::bail!(
            "organization source is not a regular file or directory: {}",
            source.display()
        );
    }

    let output = destination.resolve_under(output_root)?;
    match std::fs::symlink_metadata(&output) {
        Ok(existing) if existing.is_dir() && !existing.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!(
            "organization directory destination already exists as a non-directory: {}",
            output.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&output).with_context(|| {
                format!(
                    "creating organization output directory {}",
                    output.display()
                )
            })?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspecting organization output {}", output.display()));
        }
    }
    destination.resolve_under(output_root)?;

    let mut entries = std::fs::read_dir(source)
        .with_context(|| format!("reading organization source directory {}", source.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().into_string().map_err(|name| {
            anyhow::anyhow!(
                "organization source contains a non-Unicode path component: {:?}",
                name
            )
        })?;
        let child_destination =
            CheckedRelativePath::new(format!("{}/{}", destination.as_path().display(), name))?;
        copy_plan_source(
            output_root,
            &child_destination,
            &entry.path(),
            deferred_metadata,
        )?;
    }

    deferred_metadata.push(DeferredPlanMetadata {
        destination: destination.clone(),
        metadata: preserved_metadata,
        is_directory: true,
    });

    Ok(())
}

/// Reapply file and directory metadata after every planned child has been
/// populated and after any top-level promotion. Files are applied before
/// directories; directories are applied deepest-first so a read-only ancestor
/// cannot block a child. ACLs, extended attributes, and hard-link identity are
/// not represented by Rust's portable filesystem metadata API. Creation time
/// is preserved on Windows and macOS, but is not portable on other targets.
pub(crate) fn apply_deferred_plan_metadata(
    output_root: &Path,
    deferred_metadata: &mut [DeferredPlanMetadata],
) -> Result<()> {
    deferred_metadata.sort_by_key(|record| {
        (
            record.is_directory,
            std::cmp::Reverse(record.destination.as_path().components().count()),
        )
    });

    for record in deferred_metadata {
        let destination = record.destination.resolve_under(output_root)?;
        let entry = open_plan_metadata_handle(&destination).with_context(|| {
            format!(
                "opening organization output {} for metadata",
                destination.display()
            )
        })?;
        record
            .metadata
            .apply_to_file(&entry, &destination)
            .with_context(|| {
                format!(
                    "preserving organization metadata on {}",
                    destination.display()
                )
            })?;
    }

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
///
/// If `profile` is provided, uses its settings for compression.
/// Otherwise falls back to default 7z maximum compression.
pub fn execute_organization_plan(
    archive: &Archive,
    dest: &Path,
    plan: &crate::features::organization::engine::OrganizationPlan,
    temp_dir: &Path,
    profile: Option<&super::ArchiveProfile>,
) -> Result<()> {
    plan.validate_paths()?;

    let format_name = profile.map(|p| p.format.display_name()).unwrap_or("7z");
    info!(
        "Executing organization plan '{}' for archive {} (output: {})",
        plan.rule_name,
        archive.path().display(),
        format_name
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

    let root_folder = CheckedRelativePath::new(&plan.root_folder)?;
    root_folder.resolve_under(&organized_dir)?;
    let checked_moves = plan
        .moves
        .iter()
        .map(|(source, destination)| {
            Ok((
                CheckedRelativePath::new(source)?,
                CheckedRelativePath::new(destination)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let checked_generated = plan
        .generated_files
        .iter()
        .map(|(path, _)| CheckedRelativePath::new(path))
        .collect::<Result<Vec<_>>>()?;
    let checked_downloads = plan
        .downloads
        .iter()
        .map(|download| CheckedRelativePath::new(&download.dest_path))
        .collect::<Result<Vec<_>>>()?;

    // Reject static symlinked parents in a pre-existing work directory before
    // extraction or organization mutates it.
    for (source, destination) in &checked_moves {
        source.resolve_under(&source_extracted)?;
        destination.resolve_under(&organized_dir)?;
    }
    for path in &checked_generated {
        path.resolve_under(&organized_dir)?;
    }
    for path in &checked_downloads {
        path.resolve_under(&organized_dir)?;
    }

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
    let mut deferred_metadata = Vec::new();

    if plan.use_standard_layout {
        // Standard Layout: Smart Flattening
        // We ignore explicit moves for game content and use the flattener
        let game_path = CheckedRelativePath::new(format!("{}/Game", plan.root_folder))?;
        let game_dir = game_path.resolve_under(&organized_dir)?;
        std::fs::create_dir_all(&game_dir)?;
        let game_dir = game_path.resolve_under(&organized_dir)?;

        debug!(
            "Using Standard Layout - Flattening game content to {:?}",
            game_dir
        );
        super::flatten::find_and_flatten_game_content(&source_extracted, &game_dir)?;
    } else {
        // Legacy/Custom Layout: Explicit moves
        for ((src_rel, _), (source, destination)) in plan.moves.iter().zip(&checked_moves) {
            let src_path = source.resolve_under(&source_extracted)?;
            match std::fs::symlink_metadata(&src_path) {
                Ok(_) => {
                    let src_path = source.resolve_under(&source_extracted)?;
                    copy_plan_source(
                        &organized_dir,
                        destination,
                        &src_path,
                        &mut deferred_metadata,
                    )?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    debug!("Source file not found (maybe directory?): {}", src_rel);
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting move source {}", src_path.display()));
                }
            }
        }
    }

    // 2b. Write generated files (e.g. metadata.json)
    debug!("Writing generated files");
    for ((_, content), checked_path) in plan.generated_files.iter().zip(&checked_generated) {
        let mut bytes = std::io::Cursor::new(content.as_bytes());
        persist_plan_output(&organized_dir, checked_path, &mut bytes)?;
    }

    // 2c. Download files (e.g. screenshots)
    if !plan.downloads.is_empty() {
        debug!("Downloading {} files", plan.downloads.len());
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Arclain/1.0")
            .build()?;

        for (download, checked_path) in plan.downloads.iter().zip(&checked_downloads) {
            let dst_path = checked_path.resolve_under(&organized_dir)?;
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            debug!("Downloading {} to {}", download.url, download.dest_path);
            match client.get(&download.url).send() {
                Ok(resp) => {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes() {
                            let mut bytes = std::io::Cursor::new(bytes);
                            if let Err(e) =
                                persist_plan_output(&organized_dir, checked_path, &mut bytes)
                            {
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

    apply_deferred_plan_metadata(&organized_dir, &mut deferred_metadata)
        .context("preserving organized metadata")?;

    // 3. Compress organized directory to dest
    debug!("Compressing organized structure to {}", format_name);
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
    if let Some(profile) = profile {
        archive
            .backend()
            .create_archive_with_profile(&dest_abs, &items_to_compress, profile)
            .context("creating organized archive with profile")?;
    } else {
        // Fallback to default 7z
        archive
            .backend()
            .create_archive(&dest_abs, &items_to_compress, "7z")
            .context("creating organized 7z archive")?;
    }

    info!("Plan execution completed successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn persist_plan_output_does_not_replace_existing_leaf() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("output.bin"), b"known-good").unwrap();
        let relative = CheckedRelativePath::new("output.bin").unwrap();
        let mut replacement = Cursor::new(b"replacement");

        assert!(persist_plan_output(&root, &relative, &mut replacement).is_err());
        assert_eq!(
            std::fs::read(root.join("output.bin")).unwrap(),
            b"known-good"
        );
    }

    #[cfg(unix)]
    fn symlink_file_for_test(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn symlink_file_for_test(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_file(target, link)
            .expect("Windows symlink support is required for this containment regression");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn persist_plan_output_does_not_follow_existing_leaf_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside.bin");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&outside, b"known-good").unwrap();
        symlink_file_for_test(&outside, &root.join("output.bin"));
        let relative = CheckedRelativePath::new("output.bin").unwrap();
        let mut replacement = Cursor::new(b"replacement");

        assert!(persist_plan_output(&root, &relative, &mut replacement).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"known-good");
        assert!(std::fs::symlink_metadata(root.join("output.bin"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
