use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, error, info};

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
