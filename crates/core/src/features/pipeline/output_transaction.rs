use super::types::OutputArtifact;
use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Weak};

type DestinationMutex = Mutex<()>;

static DESTINATION_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Weak<DestinationMutex>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A destination-sibling transaction for one complete pipeline artifact.
///
/// The artifact and any prior output stay on the destination filesystem, so
/// promotion and rollback use same-filesystem renames instead of partial
/// copies over the live path.
pub(super) struct StagedOutput {
    destination: PathBuf,
    lock_key: PathBuf,
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
        let canonical_parent = parent.canonicalize().with_context(|| {
            format!(
                "canonicalizing pipeline output parent directory {}",
                parent.display()
            )
        })?;
        let lock_key = destination_lock_key(
            &canonical_parent.join(
                destination
                    .file_name()
                    .expect("destination final component was validated"),
            ),
        );

        Ok(Self {
            destination: destination.to_path_buf(),
            lock_key,
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
        self.commit_with_hooks(
            |from, to| std::fs::rename(from, to),
            |from, to| rename_noreplace(from, to),
            |_| Ok(()),
            |root| root.close().map_err(Into::into),
            |message| tracing::warn!("{message}"),
        )
    }

    #[cfg(test)]
    fn commit_with(
        self,
        rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<PathBuf> {
        let rename = std::cell::RefCell::new(rename);
        self.commit_with_hooks(
            |from, to| (rename.borrow_mut())(from, to),
            |from, to| (rename.borrow_mut())(from, to),
            |_| Ok(()),
            |root| root.close().map_err(Into::into),
            |message| tracing::warn!("{message}"),
        )
    }

    fn commit_with_hooks(
        mut self,
        mut rename_existing: impl FnMut(&Path, &Path) -> std::io::Result<()>,
        mut rename_without_replacement: impl FnMut(&Path, &Path) -> std::io::Result<()>,
        mut before_promote: impl FnMut(&Path) -> Result<()>,
        close: impl FnMut(tempfile::TempDir) -> Result<()>,
        diagnostic: impl FnMut(String),
    ) -> Result<PathBuf> {
        let destination_lock = DestinationLock::acquire(self.lock_key.clone());
        let _destination_guard = destination_lock.mutex.lock();
        let mut committed_destination = None;

        let outcome = (|| {
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
                if let Err(error) = rename_existing(&self.destination, &previous) {
                    anyhow::bail!(
                        "failed to move existing pipeline output {} into transaction backup {}: {}",
                        self.destination.display(),
                        previous.display(),
                        error
                    );
                }
            }

            let finalize_result = before_promote(&self.destination).and_then(|()| {
                rename_without_replacement(&self.artifact, &self.destination).with_context(|| {
                    format!(
                        "atomically promoting staged output to {} without replacing a concurrent arrival",
                        self.destination.display()
                    )
                })
            });

            match finalize_result {
                Ok(()) => {
                    let destination = self.destination.clone();
                    committed_destination = Some(destination.clone());
                    Ok(destination)
                }
                Err(finalize_error) if destination_exists => {
                    match rename_without_replacement(&previous, &self.destination) {
                        Ok(()) => anyhow::bail!(
                            "failed to finalize pipeline output {}: {:#}; previous output was restored",
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
                                "pipeline output was not committed: final rename to {} failed: {:#}; \
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
                    "failed to finalize new pipeline output {}: {:#}; no prior output was changed",
                    self.destination.display(),
                    finalize_error
                ),
            }
        })();

        let Some(root) = self.root.take() else {
            return outcome;
        };
        close_transaction_root(
            root,
            committed_destination.as_deref(),
            outcome,
            close,
            diagnostic,
        )
    }
}

struct DestinationLock {
    key: PathBuf,
    mutex: Arc<DestinationMutex>,
}

impl DestinationLock {
    fn acquire(key: PathBuf) -> Self {
        let mut registry = DESTINATION_LOCKS.lock();
        let mutex = match registry.get(&key).and_then(Weak::upgrade) {
            Some(mutex) => mutex,
            None => {
                let mutex = Arc::new(Mutex::new(()));
                registry.insert(key.clone(), Arc::downgrade(&mutex));
                mutex
            }
        };
        drop(registry);
        Self { key, mutex }
    }
}

impl Drop for DestinationLock {
    fn drop(&mut self) {
        let mut registry = DESTINATION_LOCKS.lock();
        let is_current_entry = registry
            .get(&self.key)
            .is_some_and(|entry| entry.as_ptr() == Arc::as_ptr(&self.mutex));
        if is_current_entry && Arc::strong_count(&self.mutex) == 1 {
            registry.remove(&self.key);
        }
    }
}

#[cfg(windows)]
fn destination_lock_key(path: &Path) -> PathBuf {
    PathBuf::from(path.as_os_str().to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
fn destination_lock_key(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
}

#[cfg(windows)]
fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    // MoveFileExW only replaces a destination when MOVEFILE_REPLACE_EXISTING
    // is set. Both paths are siblings, so this remains a same-volume rename.
    // SAFETY: both vectors are NUL-terminated UTF-16 paths, remain alive for
    // the call, and MoveFileExW only reads through their pointers.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
)))]
fn rename_noreplace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this target",
    ))
}

fn close_transaction_root<T>(
    root: tempfile::TempDir,
    committed_destination: Option<&Path>,
    outcome: Result<T>,
    mut close: impl FnMut(tempfile::TempDir) -> Result<()>,
    mut diagnostic: impl FnMut(String),
) -> Result<T> {
    let cleanup_path = root.path().to_path_buf();
    if let Err(error) = close(root) {
        let operation_state = match committed_destination {
            Some(destination) => format!("Output committed at {}", destination.display()),
            None => "Output was not committed".to_string(),
        };
        diagnostic(format!(
            "[pipeline] {operation_state}; transaction cleanup failed at {}: {error:#}",
            cleanup_path.display()
        ));
    }
    outcome
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        let Some(root) = self.root.take() else {
            return;
        };
        let _ = close_transaction_root(
            root,
            None,
            Ok(()),
            |root| root.close().map_err(Into::into),
            |message| tracing::warn!("{message}"),
        );
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
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

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

    /// Deterministically model POSIX rename semantics on every test platform:
    /// an existing file or empty directory at the destination is replaced.
    fn emulate_clobbering_rename(from: &Path, to: &Path) -> std::io::Result<()> {
        match std::fs::symlink_metadata(to) {
            Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir(to)?,
            Ok(_) => std::fs::remove_file(to)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::rename(from, to)
    }

    #[test]
    fn existence_check_followed_by_posix_style_rename_clobbers_new_targets() {
        let temp = tempfile::tempdir().unwrap();

        for destination_name in ["concurrent-file", "concurrent-directory"] {
            let source = temp.path().join(format!("source-{destination_name}"));
            let destination = temp.path().join(destination_name);
            std::fs::write(&source, b"replacement").unwrap();
            assert!(!path_exists(&destination).unwrap());

            if destination_name.ends_with("file") {
                std::fs::write(&destination, b"concurrent-arrival").unwrap();
            } else {
                std::fs::create_dir(&destination).unwrap();
            }

            emulate_clobbering_rename(&source, &destination).unwrap();
            assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
        }
    }

    #[test]
    fn concurrent_file_arrival_is_not_clobbered_when_replacement_is_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let result = staged.commit_with_hooks(
            |from, to| std::fs::rename(from, to),
            |from, to| rename_noreplace(from, to),
            |destination| {
                std::fs::write(destination, b"concurrent-arrival")?;
                Ok(())
            },
            |root| root.close().map_err(Into::into),
            |_| {},
        );

        assert!(result.is_err(), "concurrent arrival must stop promotion");
        assert_eq!(std::fs::read(&destination).unwrap(), b"concurrent-arrival");
    }

    #[test]
    fn concurrent_empty_directory_arrival_is_not_clobbered() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game");
        let staged = StagedOutput::new(&destination, false).unwrap();
        std::fs::create_dir(staged.artifact_path()).unwrap();
        let result = staged.commit_with_hooks(
            |from, to| std::fs::rename(from, to),
            |from, to| rename_noreplace(from, to),
            |destination| {
                std::fs::create_dir(destination)?;
                Ok(())
            },
            |root| root.close().map_err(Into::into),
            |_| {},
        );

        assert!(result.is_err(), "concurrent arrival must stop promotion");
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_arrival_after_backup_is_preserved_with_recovery_data() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let result = staged.commit_with_hooks(
            |from, to| std::fs::rename(from, to),
            |from, to| rename_noreplace(from, to),
            |destination| {
                std::fs::write(destination, b"concurrent-arrival")?;
                Ok(())
            },
            |root| root.close().map_err(Into::into),
            |_| {},
        );

        assert!(result.is_err(), "concurrent arrival must stop promotion");
        assert_eq!(std::fs::read(&destination).unwrap(), b"concurrent-arrival");
        let retained = transaction_dirs(temp.path());
        assert_eq!(retained.len(), 1, "recovery directory must be retained");
        assert_eq!(
            std::fs::read(retained[0].join("previous")).unwrap(),
            b"previous"
        );
        assert_eq!(
            std::fs::read(retained[0].join("artifact")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn commits_to_the_same_destination_are_serialized_and_lock_entry_is_reclaimed() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"initial").unwrap();
        let first = StagedOutput::new(&destination, true).unwrap();
        let lock_key = first.lock_key.clone();
        std::fs::write(first.artifact_path(), b"first").unwrap();
        let second = StagedOutput::new(&destination, true).unwrap();
        std::fs::write(second.artifact_path(), b"second").unwrap();

        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first_thread = std::thread::spawn(move || {
            first.commit_with_hooks(
                |from, to| std::fs::rename(from, to),
                |from, to| rename_noreplace(from, to),
                |_| {
                    first_entered_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    Ok(())
                },
                |root| root.close().map_err(Into::into),
                |_| {},
            )
        });
        first_entered_rx.recv().unwrap();
        let contender = DestinationLock::acquire(lock_key.clone());
        assert!(
            contender.mutex.try_lock().is_none(),
            "commit must hold the destination lock after backup and before promotion"
        );
        drop(contender);

        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_thread = std::thread::spawn(move || {
            second_started_tx.send(()).unwrap();
            second.commit_with_hooks(
                |from, to| std::fs::rename(from, to),
                |from, to| rename_noreplace(from, to),
                |_| {
                    second_entered_tx.send(()).unwrap();
                    Ok(())
                },
                |root| root.close().map_err(Into::into),
                |_| {},
            )
        });
        second_started_rx.recv().unwrap();
        assert!(matches!(
            second_entered_rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        release_first_tx.send(()).unwrap();
        first_thread.join().unwrap().unwrap();
        second_entered_rx.recv().unwrap();
        second_thread.join().unwrap().unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"second");
        assert!(
            !DESTINATION_LOCKS.lock().contains_key(&lock_key),
            "the destination lock registry must not retain stale entries"
        );
    }

    #[test]
    fn cleanup_failure_after_commit_is_diagnostic_only() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        let staged = StagedOutput::new(&destination, false).unwrap();
        let cleanup_path = staged.root.as_ref().unwrap().path().to_path_buf();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let diagnostics = RefCell::new(Vec::new());

        let result = staged.commit_with_hooks(
            |from, to| std::fs::rename(from, to),
            |from, to| rename_noreplace(from, to),
            |_| Ok(()),
            |_root| anyhow::bail!("injected cleanup failure"),
            |message| diagnostics.borrow_mut().push(message),
        );

        assert_eq!(result.unwrap(), destination);
        assert_eq!(std::fs::read(&destination).unwrap(), b"replacement");
        let diagnostics = diagnostics.into_inner();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("Output committed at"));
        assert!(diagnostics[0].contains(&cleanup_path.display().to_string()));
        assert!(diagnostics[0].contains("injected cleanup failure"));
    }

    #[test]
    fn cleanup_failure_before_commit_preserves_the_operation_error() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("game.zip");
        std::fs::write(&destination, b"previous").unwrap();
        let staged = StagedOutput::new(&destination, false).unwrap();
        let cleanup_path = staged.root.as_ref().unwrap().path().to_path_buf();
        std::fs::write(staged.artifact_path(), b"replacement").unwrap();
        let diagnostics = RefCell::new(Vec::new());

        let error = staged
            .commit_with_hooks(
                |from, to| std::fs::rename(from, to),
                |from, to| rename_noreplace(from, to),
                |_| Ok(()),
                |_root| anyhow::bail!("injected cleanup failure"),
                |message| diagnostics.borrow_mut().push(message),
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
        let diagnostics = diagnostics.into_inner();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("Output was not committed"));
        assert!(diagnostics[0].contains(&cleanup_path.display().to_string()));
        assert!(diagnostics[0].contains("injected cleanup failure"));
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
