use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use tracing::debug;

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

/// Pack every top-level item under `work_dir` into `dest`, using the
/// archive's own backend so the output honours the same tool the input
/// was read with.
///
/// The caller has already laid `work_dir` out -- extract, resolve the
/// plan's downloads, then apply the plan -- so this only compresses what
/// it finds. If `profile` is provided, its settings decide the format
/// and compression; otherwise the output is a default 7z.
pub fn pack_work_dir(
    archive: &Archive,
    dest: &Path,
    work_dir: &Path,
    profile: Option<&super::ArchiveProfile>,
) -> Result<()> {
    let format_name = profile.map(|p| p.format.display_name()).unwrap_or("7z");
    debug!("Compressing organized structure to {}", format_name);

    let dest_abs = if dest.is_absolute() {
        dest.to_path_buf()
    } else {
        std::env::current_dir()?.join(dest)
    };

    let mut items_to_compress = Vec::new();
    for entry in std::fs::read_dir(work_dir)
        .with_context(|| format!("reading organized directory {}", work_dir.display()))?
    {
        items_to_compress.push(entry?.path());
    }
    if items_to_compress.is_empty() {
        anyhow::bail!("organized directory is empty, nothing to compress");
    }

    match profile {
        Some(profile) => archive
            .backend()
            .create_archive_with_profile(&dest_abs, &items_to_compress, profile)
            .context("creating organized archive with profile"),
        None => archive
            .backend()
            .create_archive(&dest_abs, &items_to_compress, "7z")
            .context("creating organized 7z archive"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[cfg(windows)]
    fn suppress_automatic_access_time_update_for_test(file: &std::fs::File) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::Storage::FileSystem::SetFileTime;

        let unchanged = FILETIME {
            dwLowDateTime: u32::MAX,
            dwHighDateTime: u32::MAX,
        };
        let result = unsafe {
            SetFileTime(
                file.as_raw_handle() as _,
                std::ptr::null(),
                &unchanged,
                std::ptr::null(),
            )
        };
        assert_ne!(result, 0, "failed to protect Windows access time");
    }

    #[cfg(windows)]
    #[test]
    fn preserved_plan_metadata_applies_past_access_time_on_protected_handle() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        let initial = std::fs::metadata(&destination).unwrap();
        let expected_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200);
        let expected_accessed = expected_modified + std::time::Duration::from_secs(60);
        let preserved = PreservedPlanMetadata {
            permissions: initial.permissions(),
            accessed: expected_accessed,
            modified: expected_modified,
            created: initial.created().unwrap(),
        };

        // Opening a directory with an old access time can itself advance that
        // time on NTFS. Protect this handle before restoration, then inspect
        // through the same handle so the assertion does not mutate its subject.
        let handle = open_plan_metadata_handle(&destination).unwrap();
        suppress_automatic_access_time_update_for_test(&handle);
        preserved.apply_to_file(&handle, &destination).unwrap();

        let restored = handle.metadata().unwrap();
        assert_eq!(restored.accessed().unwrap(), expected_accessed);
        assert_eq!(restored.modified().unwrap(), expected_modified);
    }

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
