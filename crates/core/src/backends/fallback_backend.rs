use crate::archive::{is_archive_not_found, require_archive_file};
use crate::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// The context an "both tiers refused" failure carries, quoting why the
/// primary tier bowed out.
///
/// The primary is the tier that inspects the archive's own headers, so
/// its refusal is often the only statement of *why* -- "these contents
/// are password-protected", say -- while the CLI tier behind it can only
/// report its own exit status. Dropping the primary's reason leaves a
/// caller with a diagnostic that names no cause it can act on.
fn both_failed_context(
    operation: &str,
    primary_name: &str,
    primary_error: &anyhow::Error,
) -> String {
    format!("Both backends failed to {operation} ({primary_name}: {primary_error:#})")
}

/// A backend that tries a primary backend first, then falls back to a secondary backend if it fails
pub struct FallbackBackend {
    primary: Arc<dyn ArchiveBackend>,
    fallback: Arc<dyn ArchiveBackend>,
    primary_name: String,
    fallback_name: String,
}

impl FallbackBackend {
    pub fn new(primary: Arc<dyn ArchiveBackend>, fallback: Arc<dyn ArchiveBackend>) -> Self {
        let primary_name = primary.name().to_string();
        let fallback_name = fallback.name().to_string();
        Self {
            primary,
            fallback,
            primary_name,
            fallback_name,
        }
    }
}

impl ArchiveBackend for FallbackBackend {
    fn name(&self) -> &str {
        // Return the primary backend name since that's what we try first
        &self.primary_name
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Return the union of capabilities from both backends
        // If either can do something, we can do it
        let primary_caps = self.primary.capabilities();
        let fallback_caps = self.fallback.capabilities();

        BackendCapabilities {
            can_extract: primary_caps.can_extract || fallback_caps.can_extract,
            can_create: primary_caps.can_create || fallback_caps.can_create,
            can_add_files: primary_caps.can_add_files || fallback_caps.can_add_files,
            can_delete_files: primary_caps.can_delete_files || fallback_caps.can_delete_files,
            can_modify_files: primary_caps.can_modify_files || fallback_caps.can_modify_files,
            can_recompress_7z: primary_caps.can_recompress_7z || fallback_caps.can_recompress_7z,
            can_convert_to_7z: primary_caps.can_convert_to_7z || fallback_caps.can_convert_to_7z,
        }
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        match self.primary.identify(path) {
            Ok(kind) => Ok(kind),
            Err(_) => self.fallback.identify(path),
        }
    }

    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo> {
        require_archive_file(path)?;
        info!(
            "Trying {} backend to list: {}",
            self.primary_name,
            path.display()
        );

        match self.primary.list(path, password) {
            Ok(info) => {
                if info.entries.is_empty() && !info.headers_encrypted {
                    warn!(
                        "{} backend returned empty list for: {}. Assuming failure/encryption issue. Falling back to {}",
                        self.primary_name,
                        path.display(),
                        self.fallback_name
                    );
                    info!(
                        "Using fallback {} backend to list: {}",
                        self.fallback_name,
                        path.display()
                    );
                    self.fallback.list(path, password).with_context(|| {
                        format!(
                            "Both {} and {} backends failed to list archive (Primary returned empty list)",
                            self.primary_name, self.fallback_name
                        )
                    })
                } else {
                    if info.entries.is_empty() && info.headers_encrypted {
                        info!(
                            "{} backend detected encrypted headers for: {}",
                            self.primary_name,
                            path.display()
                        );
                    } else {
                        info!(
                            "Successfully listed archive with {} backend",
                            self.primary_name
                        );
                    }
                    Ok(info)
                }
            }
            // A file that vanished between the check above and the primary's
            // open is still a missing file: the fallback cannot succeed either.
            Err(e) if is_archive_not_found(&e) => Err(e),
            Err(e) => {
                warn!(
                    "{} backend failed to list archive: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                info!(
                    "Using fallback {} backend to list: {}",
                    self.fallback_name,
                    path.display()
                );
                self.fallback.list(path, password).with_context(|| {
                    format!(
                        "Both {} and {} backends failed to list archive",
                        self.primary_name, self.fallback_name
                    )
                })
            }
        }
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        require_archive_file(path)?;
        info!(
            "Trying {} backend to extract: {} to {}",
            self.primary_name,
            path.display(),
            dest.display()
        );

        match self.primary.extract_all(path, dest, password) {
            Ok(()) => {
                info!(
                    "Successfully extracted archive with {} backend",
                    self.primary_name
                );
                Ok(())
            }
            Err(e) if is_archive_not_found(&e) => Err(e),
            Err(e) => {
                warn!(
                    "{} backend failed to extract: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                info!(
                    "Using fallback {} backend to extract: {}",
                    self.fallback_name,
                    path.display()
                );
                self.fallback
                    .extract_all(path, dest, password)
                    .with_context(|| {
                        both_failed_context("extract the archive", &self.primary_name, &e)
                    })
            }
        }
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        match self.primary.extract_files(path, dest, files, password) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(
                    "{} backend failed to extract files: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .extract_files(path, dest, files, password)
                    .with_context(|| both_failed_context("extract files", &self.primary_name, &e))
            }
        }
    }

    fn extract_files_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        info!(
            "Trying {} backend to extract {} files with progress",
            self.primary_name,
            files.len()
        );

        match self
            .primary
            .extract_files_with_progress(path, dest, files, password, progress, cancel)
        {
            Ok(()) => {
                info!(
                    "Successfully extracted {} files with {} backend",
                    files.len(),
                    self.primary_name
                );
                Ok(())
            }
            Err(e) => {
                // Don't fallback if cancelled
                if e.to_string().contains("cancelled") {
                    return Err(e);
                }
                warn!(
                    "{} backend failed to extract files: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .extract_files_with_progress(path, dest, files, password, progress, cancel)
                    .with_context(|| both_failed_context("extract files", &self.primary_name, &e))
            }
        }
    }

    fn extract_directory(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<()> {
        match self
            .primary
            .extract_directory(path, dest, dir_path, password)
        {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(
                    "{} backend failed to extract directory: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .extract_directory(path, dest, dir_path, password)
                    .with_context(|| {
                        both_failed_context("extract directory", &self.primary_name, &e)
                    })
            }
        }
    }

    fn extract(
        &self,
        archive: &Path,
        dest: &Path,
        entries: Option<&[crate::EntryRef<'_>]>,
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        info!(
            "Trying {} backend unified extract (entries: {:?})",
            self.primary_name,
            entries.map(|e| e.len())
        );

        match self
            .primary
            .extract(archive, dest, entries, password, progress, cancel)
        {
            Ok(()) => {
                info!("Successfully extracted with {} backend", self.primary_name);
                Ok(())
            }
            Err(e) => {
                // Don't fallback if cancelled
                if e.to_string().contains("cancelled") {
                    return Err(e);
                }
                warn!(
                    "{} backend failed unified extract: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .extract(archive, dest, entries, password, progress, cancel)
                    .with_context(|| both_failed_context("extract", &self.primary_name, &e))
            }
        }
    }

    fn extract_entry_to_writer(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
        writer: &mut dyn std::io::Write,
    ) -> Result<()> {
        // OPTIMIZATION: Skip the native 7z backend for single-entry streaming.
        //
        // The sevenz-rust2 library has a bug with solid archives where early termination
        // of the for_each_entries() iterator (returning Ok(false) to stop after finding
        // the target file) causes ChecksumVerificationFailed errors. This happens because
        // solid archives compress multiple files together in blocks, and the library
        // validates the entire block even when we only want one file.
        //
        // Diagnosis confirmed via logs:
        // - Native backend ALWAYS fails after ~1.9s with ChecksumVerificationFailed
        // - CLI fallback succeeds in ~0.45s
        // - Total time per file was ~2.3s (wasted 1.9s + 0.45s)
        //
        // By going directly to CLI, we reduce per-file extraction from ~2.3s to ~0.45s.
        // The CLI backend uses `7z e -so` which streams entries efficiently.
        //
        // For bulk extraction (extract_all, extract_files), the native backend works fine
        // because it processes all entries without early termination.

        self.fallback
            .extract_entry_to_writer(archive, path_in_archive, password, writer)
    }

    fn recompress_7z(&self, source: &Path, dest_7z: &Path) -> Result<()> {
        match self.primary.recompress_7z(source, dest_7z) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.recompress_7z(source, dest_7z),
        }
    }

    fn add_files(&self, archive: &Path, files: &[PathBuf]) -> Result<()> {
        match self.primary.add_files(archive, files) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.add_files(archive, files),
        }
    }

    fn create_archive(&self, dest: &Path, files: &[PathBuf], format: &str) -> Result<()> {
        match self.primary.create_archive(dest, files, format) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.create_archive(dest, files, format),
        }
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        match self
            .primary
            .read_text_file(archive, path_in_archive, password)
        {
            Ok(content) => Ok(content),
            Err(e) => {
                warn!(
                    "{} backend failed to read file: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .read_text_file(archive, path_in_archive, password)
                    .with_context(|| both_failed_context("read file", &self.primary_name, &e))
            }
        }
    }

    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        match self.primary.delete_files(archive, files) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.delete_files(archive, files),
        }
    }

    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        match self
            .primary
            .add_or_update_file_from_str(archive, path_in_archive, content)
        {
            Ok(()) => Ok(()),
            Err(_) => self
                .fallback
                .add_or_update_file_from_str(archive, path_in_archive, content),
        }
    }

    fn convert_to_7z(&self, source: &crate::Archive, dest: &Path, temp_dir: &Path) -> Result<()> {
        match self.primary.convert_to_7z(source, dest, temp_dir) {
            Ok(()) => Ok(()),
            Err(_) => self.fallback.convert_to_7z(source, dest, temp_dir),
        }
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        match self
            .primary
            .crc32_of_entry(archive, path_in_archive, password)
        {
            Ok(crc) => Ok(crc),
            Err(e) => {
                warn!(
                    "{} backend failed to get CRC: {}. Falling back to {}",
                    self.primary_name, e, self.fallback_name
                );
                self.fallback
                    .crc32_of_entry(archive, path_in_archive, password)
                    .with_context(|| both_failed_context("get CRC", &self.primary_name, &e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{is_archive_not_found, ArchiveNotFound};
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A tier that counts how often it is asked and fails the way it was told to.
    struct CountingBackend {
        name: &'static str,
        calls: AtomicUsize,
        failure: Option<Box<dyn Fn() -> anyhow::Error + Send + Sync>>,
    }

    impl CountingBackend {
        fn succeeding(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                calls: AtomicUsize::new(0),
                failure: None,
            })
        }

        fn failing(
            name: &'static str,
            failure: impl Fn() -> anyhow::Error + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                name,
                calls: AtomicUsize::new(0),
                failure: Some(Box::new(failure)),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn answer<T>(&self, ok: impl FnOnce() -> T) -> Result<T> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.failure {
                Some(failure) => Err(failure()),
                None => Ok(ok()),
            }
        }
    }

    impl ArchiveBackend for CountingBackend {
        fn name(&self) -> &str {
            self.name
        }
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> Result<ArchiveKind> {
            Ok(ArchiveKind::Zip)
        }
        fn list(&self, path: &Path, _password: Option<&str>) -> Result<ArchiveInfo> {
            // Encrypted headers with no entries is the one empty listing the
            // chain accepts as an answer, so the fake needs no entry fixture.
            self.answer(|| ArchiveInfo {
                archive_path: path.to_path_buf(),
                archive_kind: ArchiveKind::Zip,
                entries: Vec::new(),
                encrypted: true,
                headers_encrypted: true,
                encryption_method: None,
            })
        }
        fn extract_all(&self, _path: &Path, _dest: &Path, _password: Option<&str>) -> Result<()> {
            self.answer(|| ())
        }
        fn extract_files(
            &self,
            _path: &Path,
            _dest: &Path,
            _files: &[String],
            _password: Option<&str>,
        ) -> Result<()> {
            unimplemented!()
        }
        fn extract_directory(
            &self,
            _path: &Path,
            _dest: &Path,
            _dir_path: &str,
            _password: Option<&str>,
        ) -> Result<()> {
            unimplemented!()
        }
        fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> Result<()> {
            unimplemented!()
        }
        fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> Result<()> {
            unimplemented!()
        }
        fn create_archive(&self, _dest: &Path, _files: &[PathBuf], _format: &str) -> Result<()> {
            unimplemented!()
        }
        fn read_text_file(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> Result<String> {
            unimplemented!()
        }
        fn delete_files(&self, _archive: &Path, _files: &[String]) -> Result<()> {
            unimplemented!()
        }
        fn add_or_update_file_from_str(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _content: &str,
        ) -> Result<()> {
            unimplemented!()
        }
        fn convert_to_7z(
            &self,
            _source: &crate::Archive,
            _dest: &Path,
            _temp_dir: &Path,
        ) -> Result<()> {
            unimplemented!()
        }
        fn crc32_of_entry(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> Result<String> {
            unimplemented!()
        }
    }

    /// The zip tier's failure shape: an io not-found under an anyhow context.
    fn zip_open_not_found() -> anyhow::Error {
        anyhow::Error::from(io::Error::new(io::ErrorKind::NotFound, "gone"))
            .context("Failed to open ZIP file")
    }

    #[test]
    fn missing_archive_is_answered_before_either_tier_runs() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.zip");
        let primary = CountingBackend::succeeding("primary");
        let fallback = CountingBackend::succeeding("fallback");
        let chain = FallbackBackend::new(primary.clone(), fallback.clone());

        let listing = chain.list(&missing, None).unwrap_err();
        assert!(
            listing.downcast_ref::<ArchiveNotFound>().is_some(),
            "expected the typed not-found, got {listing:#}"
        );
        let extraction = chain.extract_all(&missing, dir.path(), None).unwrap_err();
        assert!(extraction.downcast_ref::<ArchiveNotFound>().is_some());

        assert_eq!(
            primary.calls(),
            0,
            "no tier was asked about a file that is not there"
        );
        assert_eq!(fallback.calls(), 0);
    }

    #[test]
    fn a_tier_reporting_not_found_is_not_followed_by_the_fallback() {
        // The file exists at the check and is gone by the time the primary
        // opens it: the primary's io not-found is the answer, not a reason
        // to run the fallback (which cannot succeed either).
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, b"not really a zip").unwrap();
        let primary = CountingBackend::failing("primary", zip_open_not_found);
        let fallback = CountingBackend::succeeding("fallback");
        let chain = FallbackBackend::new(primary.clone(), fallback.clone());

        let error = chain.list(&archive, None).unwrap_err();

        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 0, "the fallback tier must not run");
        assert!(
            is_archive_not_found(&error),
            "the io kind stays reachable: {error:#}"
        );
    }

    #[test]
    fn other_primary_failures_still_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, b"not really a zip").unwrap();
        let primary =
            CountingBackend::failing("primary", || anyhow::anyhow!("Failed to read ZIP archive"));
        let fallback = CountingBackend::succeeding("fallback");
        let chain = FallbackBackend::new(primary.clone(), fallback.clone());

        chain.list(&archive, None).expect("the fallback answers");

        assert_eq!(primary.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[test]
    fn is_archive_not_found_reads_both_shapes_and_nothing_else() {
        assert!(is_archive_not_found(
            &ArchiveNotFound {
                path: PathBuf::from("gone.zip")
            }
            .into()
        ));
        assert!(is_archive_not_found(&zip_open_not_found()));
        assert!(!is_archive_not_found(&anyhow::anyhow!("Wrong password")));
        assert!(!is_archive_not_found(&anyhow::Error::from(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "locked"
        ))));
    }
}
