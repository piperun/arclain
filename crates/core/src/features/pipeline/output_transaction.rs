use super::types::OutputArtifact;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A destination-sibling transaction for one complete pipeline artifact.
///
/// The artifact and any prior output stay on the destination filesystem, so
/// promotion and rollback use same-filesystem renames instead of partial
/// copies over the live path.
pub(super) struct StagedOutput {
    destination: PathBuf,
    root: Option<tempfile::TempDir>,
    artifact: PathBuf,
    replace_existing: bool,
}

impl StagedOutput {
    pub(super) fn new(destination: &Path, replace_existing: bool) -> Result<Self> {
        let parent = destination
            .parent()
            .context("pipeline output destination has no parent directory")?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        destination
            .file_name()
            .context("pipeline output destination has no final path component")?;

        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "creating pipeline output parent directory {}",
                parent.display()
            )
        })?;
        let root = tempfile::Builder::new()
            .prefix(".arclain-output-")
            .tempdir_in(parent)
            .with_context(|| {
                format!(
                    "creating pipeline output transaction beside {}",
                    destination.display()
                )
            })?;
        let artifact = root.path().join("artifact");

        Ok(Self {
            destination: destination.to_path_buf(),
            root: Some(root),
            artifact,
            replace_existing,
        })
    }

    pub(super) fn artifact_path(&self) -> &Path {
        &self.artifact
    }

    pub(super) fn verify(&self, artifact: OutputArtifact) -> Result<()> {
        match artifact {
            OutputArtifact::Archive => {
                let metadata = std::fs::symlink_metadata(&self.artifact).with_context(|| {
                    format!(
                        "reading staged archive metadata at {}",
                        self.artifact.display()
                    )
                })?;
                if !metadata.file_type().is_file() {
                    anyhow::bail!(
                        "staged archive is not a regular file: {}",
                        self.artifact.display()
                    );
                }
                if metadata.len() == 0 {
                    anyhow::bail!("staged archive is empty: {}", self.artifact.display());
                }
                Ok(())
            }
            OutputArtifact::Folder => verify_regular_tree(&self.artifact),
        }
    }

    pub(super) fn commit(self) -> Result<PathBuf> {
        self.commit_with(|from, to| std::fs::rename(from, to))
    }

    fn commit_with(
        mut self,
        mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<PathBuf> {
        let destination_exists = path_exists(&self.destination).with_context(|| {
            format!(
                "checking pipeline output destination {}",
                self.destination.display()
            )
        })?;

        if destination_exists && !self.replace_existing {
            anyhow::bail!(
                "pipeline output already exists and replacement is disabled: {}",
                self.destination.display()
            );
        }

        let previous = self
            .root
            .as_ref()
            .expect("staged output transaction root is present before commit")
            .path()
            .join("previous");

        if destination_exists {
            if let Err(error) = rename(&self.destination, &previous) {
                anyhow::bail!(
                    "failed to move existing pipeline output {} into transaction backup {}: {}",
                    self.destination.display(),
                    previous.display(),
                    error
                );
            }
        }

        match rename(&self.artifact, &self.destination) {
            Ok(()) => {
                let destination = self.destination.clone();
                let root = self
                    .root
                    .take()
                    .expect("staged output transaction root is present after promotion");
                let cleanup_path = root.path().to_path_buf();
                if let Err(error) = root.close() {
                    tracing::warn!(
                        "[pipeline] Output committed at {}; transaction cleanup failed at {}: {}",
                        destination.display(),
                        cleanup_path.display(),
                        error
                    );
                }
                Ok(destination)
            }
            Err(finalize_error) if destination_exists => {
                match rename(&previous, &self.destination) {
                    Ok(()) => anyhow::bail!(
                        "failed to finalize pipeline output {}: {}; previous output was restored",
                        self.destination.display(),
                        finalize_error
                    ),
                    Err(rollback_error) => {
                        let recovery_path = self
                            .root
                            .take()
                            .expect(
                                "staged output transaction root is present after rollback failure",
                            )
                            .keep();
                        anyhow::bail!(
                            "pipeline output was not committed: final rename to {} failed: {}; \
                             restoring the previous output also failed: {}; recovery data was \
                             retained at {} (previous output: previous, replacement: artifact)",
                            self.destination.display(),
                            finalize_error,
                            rollback_error,
                            recovery_path.display()
                        );
                    }
                }
            }
            Err(finalize_error) => anyhow::bail!(
                "failed to finalize new pipeline output {}: {}; no prior output was changed",
                self.destination.display(),
                finalize_error
            ),
        }
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        let Some(root) = self.root.take() else {
            return;
        };
        let cleanup_path = root.path().to_path_buf();
        if let Err(error) = root.close() {
            tracing::warn!(
                "[pipeline] Output was not committed; transaction cleanup failed at {}: {}",
                cleanup_path.display(),
                error
            );
        }
    }
}

fn path_exists(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn verify_regular_tree(root: &Path) -> Result<()> {
    let root_metadata = std::fs::symlink_metadata(root)
        .with_context(|| format!("reading staged folder metadata at {}", root.display()))?;
    if !root_metadata.file_type().is_dir() {
        anyhow::bail!("staged folder is not a directory: {}", root.display());
    }
    verify_regular_tree_entries(root)
}

fn verify_regular_tree_entries(directory: &Path) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("reading staged output directory {}", directory.display()))?
    {
        let entry = entry.with_context(|| {
            format!("reading an entry in staged output {}", directory.display())
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading staged output type for {}", path.display()))?;
        if file_type.is_dir() {
            verify_regular_tree_entries(&path)?;
        } else if file_type.is_file() {
            std::fs::symlink_metadata(&path).with_context(|| {
                format!("reading staged output metadata for {}", path.display())
            })?;
        } else {
            anyhow::bail!(
                "staged folder contains a symlink or special filesystem node: {}",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn transaction_dirs(parent: &std::path::Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".arclain-output-"))
            })
            .collect()
    }

    #[test]
    fn failed_final_rename_restores_previous_output() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let calls = AtomicUsize::new(0);

        let result = staged.commit_with(|from, to| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected final rename failure",
                ));
            }
            std::fs::rename(from, to)
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
        assert!(transaction_dirs(temp.path()).is_empty());
    }

    #[test]
    fn failed_rollback_retains_both_artifacts_and_reports_recovery_path() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let calls = AtomicUsize::new(0);

        let error = staged
            .commit_with(|from, to| match calls.fetch_add(1, Ordering::SeqCst) {
                0 => std::fs::rename(from, to),
                1 => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected final rename failure",
                )),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected rollback failure",
                )),
            })
            .unwrap_err()
            .to_string();

        let retained = transaction_dirs(temp.path());
        assert_eq!(
            retained.len(),
            1,
            "expected one retained recovery directory"
        );
        assert_eq!(
            std::fs::read(retained[0].join("previous")).unwrap(),
            b"previous"
        );
        assert_eq!(
            std::fs::read(retained[0].join("artifact")).unwrap(),
            b"replacement"
        );
        assert!(error.contains("injected final rename failure"), "{error}");
        assert!(error.contains("injected rollback failure"), "{error}");
        assert!(
            error.contains(&retained[0].display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn replacement_disabled_leaves_existing_output_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();

        let error = staged.commit().unwrap_err().to_string();

        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
        assert!(transaction_dirs(temp.path()).is_empty());
    }

    #[test]
    fn archive_verification_rejects_empty_files_and_accepts_nonempty_files() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"known-good").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(staged.artifact_path(), b"").unwrap();

        assert!(staged.verify(OutputArtifact::Archive).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");

        std::fs::write(staged.artifact_path(), b"archive").unwrap();
        staged.verify(OutputArtifact::Archive).unwrap();
    }

    #[test]
    fn dropping_a_partial_conversion_never_touches_existing_output() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"known-good").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(staged.artifact_path(), b"partial-converter-output").unwrap();

        drop(staged);

        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");
        assert!(transaction_dirs(temp.path()).is_empty());
    }

    #[test]
    fn folder_verification_walks_the_complete_regular_tree() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game");
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::create_dir_all(staged.artifact_path().join("nested")).unwrap();
        std::fs::write(staged.artifact_path().join("root.txt"), b"root").unwrap();
        std::fs::write(staged.artifact_path().join("nested/leaf.txt"), b"leaf").unwrap();

        staged.verify(OutputArtifact::Folder).unwrap();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn folder_verification_rejects_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game");
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::create_dir(staged.artifact_path()).unwrap();
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        let link = staged.artifact_path().join("linked.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, &link)
            .expect("Windows symlink support is required for this containment regression");

        let error = staged
            .verify(OutputArtifact::Folder)
            .unwrap_err()
            .to_string();

        assert!(error.contains("symlink or special"), "{error}");
    }

    #[test]
    fn successful_archive_replacement_is_atomic_and_cleans_its_backup() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        let transaction_root = staged.root.as_ref().unwrap().path().to_path_buf();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        staged.verify(OutputArtifact::Archive).unwrap();

        assert_eq!(staged.commit().unwrap(), destination);
        assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
        assert!(!transaction_root.exists());
    }

    #[test]
    fn successful_folder_output_commits_a_complete_tree() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game");
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::create_dir_all(staged.artifact_path().join("nested")).unwrap();
        std::fs::write(staged.artifact_path().join("nested/file.txt"), b"contents").unwrap();
        staged.verify(OutputArtifact::Folder).unwrap();

        assert_eq!(staged.commit().unwrap(), destination);
        assert_eq!(
            std::fs::read(destination.join("nested/file.txt")).unwrap(),
            b"contents"
        );
    }
}
