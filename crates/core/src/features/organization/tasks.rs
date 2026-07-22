use anyhow::{Context, Result};
use base64::Engine;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use super::metadata::{GameMetadata, ScreenshotData};
use crate::Archive;

fn close_work_dir_preserving_result<T, E>(
    operation_result: std::result::Result<T, E>,
    path: &Path,
    close: impl FnOnce() -> std::io::Result<()>,
    report: impl FnOnce(&str),
) -> std::result::Result<T, E> {
    if let Err(cleanup_error) = close() {
        let message = format!(
            "Failed to cleanup owned work directory {}: {} ({:?})",
            path.display(),
            cleanup_error,
            cleanup_error.kind()
        );
        report(&message);
    }

    operation_result
}

/// A uniquely created temporary work directory with exact RAII ownership.
pub struct OwnedWorkDir(Option<tempfile::TempDir>);

impl OwnedWorkDir {
    /// Atomically create a unique child of `parent` owned by the returned value.
    pub fn new(parent: &Path, prefix: &str) -> Result<Self> {
        debug!(
            "Creating work directory with prefix '{}' in {:?}",
            prefix, parent
        );

        if prefix.is_empty()
            || !prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            anyhow::bail!(
                "work directory prefix must contain only ASCII letters, digits, '_' or '-': {prefix:?}"
            );
        }

        std::fs::create_dir_all(parent).context("creating work directory parent")?;
        let owned = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .context("creating unique work directory")?;
        info!("Created work directory: {}", owned.path().display());
        Ok(Self(Some(owned)))
    }

    pub fn path(&self) -> &Path {
        self.0
            .as_ref()
            .expect("owned work directory is present until drop")
            .path()
    }
}

impl Drop for OwnedWorkDir {
    fn drop(&mut self) {
        let Some(owned) = self.0.take() else {
            return;
        };
        let path = owned.path().to_path_buf();
        let _ = close_work_dir_preserving_result::<(), std::convert::Infallible>(
            Ok(()),
            &path,
            move || owned.close(),
            |message| error!("{message}"),
        );
    }
}

/// Create a unique temporary work directory.
pub fn create_work_directory(temp_dir: &Path, prefix: &str) -> Result<OwnedWorkDir> {
    debug!(
        "Creating work directory with prefix '{}' in {:?}",
        prefix, temp_dir
    );
    OwnedWorkDir::new(temp_dir, prefix)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::{mpsc, Arc, Barrier};

    fn sibling_names(parent: &Path) -> BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    #[test]
    fn owned_work_directory_rejects_nonportable_prefixes_without_creating_siblings() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let sentinel = temp.path().join("sentinel");
        std::fs::write(&sentinel, b"preserve").unwrap();

        for prefix in [
            "",
            "../escape-",
            "/absolute-",
            r"..\escape-",
            r"\absolute-",
            "C:drive-",
            r"\\server\share-",
        ] {
            let before = sibling_names(temp.path());
            let result = OwnedWorkDir::new(&parent, prefix);
            let after = sibling_names(temp.path());

            assert_eq!(
                after, before,
                "unsafe prefix {prefix:?} created a sibling outside the parent"
            );
            assert!(result.is_err(), "accepted unsafe prefix {prefix:?}");
        }

        assert_eq!(std::fs::read(sentinel).unwrap(), b"preserve");
    }

    #[test]
    fn owned_work_directory_accepts_portable_prefixes() {
        let temp = tempfile::tempdir().unwrap();

        for prefix in ["arc", "arclain_organize", "ARC-123"] {
            let owned = OwnedWorkDir::new(temp.path(), prefix).unwrap();
            assert!(owned
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(prefix));
        }
    }

    #[test]
    fn cleanup_failure_diagnostic_preserves_the_operation_result() {
        let path = Path::new(r"C:\test\owned-workspace");
        let original_result: std::result::Result<(), &str> = Err("original operation failure");
        let mut diagnostic = None;

        let returned_result = close_work_dir_preserving_result(
            original_result,
            path,
            || {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "permission denied by test",
                ))
            },
            |message| diagnostic = Some(message.to_owned()),
        );

        assert_eq!(returned_result.unwrap_err(), "original operation failure");
        assert_eq!(
            diagnostic.unwrap(),
            format!(
                "Failed to cleanup owned work directory {}: permission denied by test (PermissionDenied)",
                path.display()
            )
        );
    }

    #[test]
    fn owned_work_directories_with_the_same_prefix_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().to_path_buf();
        let ready = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(3));
        let (paths, received_paths) = mpsc::channel();

        let workers = (0..2)
            .map(|_| {
                let parent = parent.clone();
                let ready = Arc::clone(&ready);
                let release = Arc::clone(&release);
                let paths = paths.clone();
                std::thread::spawn(move || {
                    let work_dir = OwnedWorkDir::new(&parent, "arc").unwrap();
                    paths.send(work_dir.path().to_path_buf()).unwrap();
                    ready.wait();
                    release.wait();
                })
            })
            .collect::<Vec<_>>();
        drop(paths);

        ready.wait();
        let owned_paths = [
            received_paths.recv().unwrap(),
            received_paths.recv().unwrap(),
        ];
        assert_ne!(owned_paths[0], owned_paths[1]);
        assert!(owned_paths.iter().all(|path| path.is_dir()));

        release.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(owned_paths.iter().all(|path| !path.exists()));
    }

    #[test]
    fn dropping_one_owned_work_directory_preserves_its_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let first = OwnedWorkDir::new(temp.path(), "arc").unwrap();
        let second = OwnedWorkDir::new(temp.path(), "arc").unwrap();
        let first_path = first.path().to_path_buf();
        let second_path = second.path().to_path_buf();

        drop(first);

        assert!(!first_path.exists());
        assert!(second_path.is_dir());
    }

    #[test]
    fn owned_work_directory_never_adopts_or_deletes_a_predictable_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let predictable = temp.path().join(format!(
            "arc_{}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            std::process::id()
        ));
        std::fs::create_dir(&predictable).unwrap();
        let sentinel = predictable.join("sentinel");
        std::fs::write(&sentinel, b"preserve").unwrap();

        let owned = OwnedWorkDir::new(temp.path(), "arc").unwrap();
        assert_ne!(owned.path(), predictable);
        drop(owned);

        assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserve");
    }

    #[test]
    fn owned_work_directory_cleans_up_after_success_error_and_unwind() {
        let temp = tempfile::tempdir().unwrap();
        let sibling = temp.path().join("sibling");
        std::fs::create_dir(&sibling).unwrap();
        let sibling_sentinel = sibling.join("sentinel");
        std::fs::write(&sibling_sentinel, b"preserve").unwrap();

        let success_path = {
            let owned = OwnedWorkDir::new(temp.path(), "arc").unwrap();
            let path = owned.path().to_path_buf();
            std::fs::write(path.join("success"), b"data").unwrap();
            path
        };

        let mut error_path = None;
        let result = (|| -> Result<()> {
            let owned = OwnedWorkDir::new(temp.path(), "arc")?;
            error_path = Some(owned.path().to_path_buf());
            anyhow::bail!("expected test error");
        })();
        assert!(result.is_err());

        let mut unwind_path = None;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let owned = OwnedWorkDir::new(temp.path(), "arc").unwrap();
            unwind_path = Some(owned.path().to_path_buf());
            panic!("expected test panic");
        }));
        assert!(unwind.is_err());

        assert!(!success_path.exists());
        assert!(!error_path.unwrap().exists());
        assert!(!unwind_path.unwrap().exists());
        assert_eq!(std::fs::read(&sibling_sentinel).unwrap(), b"preserve");
    }
}
