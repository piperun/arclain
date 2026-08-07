use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use sevenz_rust2::{ArchiveReader, Password};
use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Native 7z backend using the `sevenz-rust2` crate
#[derive(Clone)]
pub struct SevenZBackend;

fn sevenz_host_error(error: anyhow::Error) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Other(Cow::Owned(format!("{error:#}")))
}

fn sevenz_host_io(error: std::io::Error, context: String) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Io(error, Cow::Owned(context))
}

fn write_archive_entry<R: Read + ?Sized>(
    dest: &Path,
    entry: &sevenz_rust2::ArchiveEntry,
    reader: &mut R,
) -> std::result::Result<(), sevenz_rust2::Error> {
    let entry_name = &entry.name;
    let relative = crate::utilities::CheckedRelativePath::new(entry_name)
        .with_context(|| format!("unsafe 7z entry {entry_name:?}"))
        .map_err(sevenz_host_error)?;
    let output = relative.resolve_under(dest).map_err(sevenz_host_error)?;

    if entry.is_directory {
        std::fs::create_dir_all(&output).map_err(|error| {
            sevenz_host_io(
                error,
                format!("create archive directory {}", output.display()),
            )
        })?;
        relative.resolve_under(dest).map_err(sevenz_host_error)?;
        return Ok(());
    }

    let parent = output
        .parent()
        .ok_or_else(|| anyhow!("archive entry has no destination parent: {entry_name:?}"))
        .map_err(sevenz_host_error)?;
    std::fs::create_dir_all(parent).map_err(|error| {
        sevenz_host_io(error, format!("create archive parent {}", parent.display()))
    })?;

    let checked_output = relative.resolve_under(dest).map_err(sevenz_host_error)?;
    let checked_parent = checked_output
        .parent()
        .ok_or_else(|| anyhow!("archive entry has no destination parent: {entry_name:?}"))
        .map_err(sevenz_host_error)?;
    let mut staged = tempfile::NamedTempFile::new_in(checked_parent).map_err(|error| {
        sevenz_host_io(
            error,
            format!("stage extracted file in {}", checked_parent.display()),
        )
    })?;

    let mut buffer = [0_u8; 64 * 1024];
    loop {
        // Archive-reader I/O must remain context-free so BlockDecoder can
        // classify encrypted read failures as MaybeBadPassword.
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        staged.write_all(&buffer[..read]).map_err(|error| {
            sevenz_host_io(error, format!("write staged archive entry {entry_name:?}"))
        })?;
    }
    staged
        .flush()
        .map_err(|error| sevenz_host_io(error, format!("flush archive entry {entry_name:?}")))?;
    staged.as_file().sync_all().map_err(|error| {
        sevenz_host_io(error, format!("sync staged archive entry {entry_name:?}"))
    })?;

    if entry.size > 0 {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::FileTimesExt;
        #[cfg(windows)]
        use std::os::windows::fs::FileTimesExt;

        let file_times = std::fs::FileTimes::new()
            .set_accessed(entry.access_date.into())
            .set_modified(entry.last_modified_date.into());

        #[cfg(any(windows, target_os = "macos"))]
        let file_times = file_times.set_created(entry.creation_date.into());

        // Match sevenz-rust2's prior default extractor: timestamps are best
        // effort and do not turn an otherwise valid extraction into failure.
        let _ = staged.as_file().set_times(file_times);
    }

    // Revalidate immediately before persistence. `persist_noclobber` atomically
    // fails if the leaf now exists, so a final-component symlink is never
    // followed or overwritten. Parent replacement by a separate local process
    // remains the documented std-only limitation.
    let checked_output = relative.resolve_under(dest).map_err(sevenz_host_error)?;
    staged
        .persist_noclobber(&checked_output)
        .map_err(|error| error.error)
        .map_err(|error| {
            sevenz_host_io(
                error,
                format!("persist extracted file {}", checked_output.display()),
            )
        })?;

    Ok(())
}

impl SevenZBackend {
    pub fn new() -> Self {
        Self
    }

    /// Renders an entry's Windows file time as the `modified` string
    /// every backend reports.
    ///
    /// A 7z header records a true instant -- 100-nanosecond ticks from
    /// 1601 -- rather than the zone-less wall clock a ZIP or RAR header
    /// carries, so it is rendered in UTC and converts back to that very
    /// instant on any machine in any zone. 7-Zip's own GUI shows the same
    /// instant in the viewer's local time, so the two read the same
    /// moment offset from each other by that zone's UTC offset.
    fn format_time(nt_time: sevenz_rust2::NtTime) -> Option<String> {
        /// Ticks of the archive's clock in one second.
        const TICKS_PER_SECOND: i128 = 10_000_000;

        // The crate's own constant rather than a hardcoded 1601-to-1970
        // tick count, and `i128` throughout so an entry predating 1970
        // (which the 1601 epoch permits) subtracts without wrapping.
        let ticks = i128::from(u64::from(nt_time));
        let epoch = i128::from(u64::from(sevenz_rust2::NtTime::UNIX_EPOCH));
        // Euclidean division so a pre-1970 instant floors toward the
        // earlier second rather than truncating toward the epoch.
        let seconds = i64::try_from((ticks - epoch).div_euclid(TICKS_PER_SECOND)).ok()?;

        crate::backends::entry_time::from_unix_seconds(seconds)
    }
}

impl ArchiveBackend for SevenZBackend {
    fn name(&self) -> &str {
        "7z (Native)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // 7z backend supports full read/write operations
        BackendCapabilities::full_featured()
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "7z" | "exe" | "sfx" => Ok(ArchiveKind::SevenZ),
            _ => Err(anyhow!("Not a 7z archive: {}", path.display())),
        }
    }

    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo> {
        info!("Using {} backend to list: {}", self.name(), path.display());

        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);

        let reader = ArchiveReader::open(path, pwd).context("Failed to open 7z archive")?;
        let archive = reader.archive();

        let mut entries = Vec::with_capacity(archive.files.len());
        let mut any_encrypted = false;
        let headers_encrypted = false; // 7z-rust2 doesn't easily expose this

        for (index, entry) in archive.files.iter().enumerate() {
            let is_dir = entry.is_directory;
            // An entry is encrypted iff the block carrying its stream
            // decodes through an AES coder. A streamless entry -- a
            // directory, an empty file -- maps to no block and has
            // nothing for a cipher to cover. (Having a stream and a CRC
            // is NOT a signal: nearly every stored file has both,
            // encrypted or not.)
            let encrypted = archive
                .stream_map
                .file_block_index
                .get(index)
                .copied()
                .flatten()
                .and_then(|block_index| archive.blocks.get(block_index))
                .is_some_and(|block| {
                    block.coders.iter().any(|coder| {
                        coder.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256
                    })
                });

            if encrypted {
                any_encrypted = true;
            }

            let modified = if entry.has_last_modified_date {
                Self::format_time(entry.last_modified_date)
            } else {
                None
            };

            entries.push(ArchiveEntry {
                path: entry.name.clone(),
                size: entry.size,
                packed_size: entry.compressed_size,
                modified,
                is_dir,
                encrypted,
                crc32: if entry.has_crc {
                    Some(format!("{:08X}", entry.crc))
                } else {
                    None
                },
            });
        }

        let encryption_method = if any_encrypted {
            Some("7z".to_string())
        } else {
            None
        };

        Ok(ArchiveInfo {
            archive_path: path.to_path_buf(),
            archive_kind: ArchiveKind::SevenZ,
            entries,
            encrypted: any_encrypted,
            headers_encrypted,
            encryption_method,
        })
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Using {} backend to extract {} to {}",
            self.name(),
            path.display(),
            dest.display()
        );

        std::fs::create_dir_all(dest).context("Failed to create destination directory")?;

        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);
        let mut reader = ArchiveReader::open(path, pwd).context("Failed to open 7z archive")?;
        reader
            .for_each_entries(|entry, reader| {
                write_archive_entry(dest, entry, reader)?;
                Ok(true)
            })
            .context("Failed to extract 7z archive")?;

        Ok(())
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        self.extract_files_with_progress(path, dest, files, password, None, None)
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
            "Using {} backend to extract {} files with progress",
            self.name(),
            files.len()
        );

        std::fs::create_dir_all(dest)?;

        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);

        let mut reader = ArchiveReader::open(path, pwd)?;

        // Count total for progress
        let total = files.len();
        let mut processed = 0;

        reader.for_each_entries(|entry, reader| {
            // Check for cancellation
            if let Some(token) = cancel {
                if token.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("Extraction cancelled");
                    return Err(sevenz_rust2::Error::Other(
                        "Extraction cancelled by user".into(),
                    ));
                }
            }

            let entry_path = entry.name.clone();
            if files
                .iter()
                .any(|f| entry_path == *f || entry_path.contains(f))
            {
                // Update progress BEFORE processing to show current file
                processed += 1;
                if let Some(cb) = progress {
                    let percent = if total > 0 {
                        ((processed * 100) / total) as u8
                    } else {
                        0
                    };
                    cb(crate::ExtractionProgress {
                        current: processed,
                        total,
                        current_file: entry_path.clone(),
                        percent,
                    });
                }

                write_archive_entry(dest, entry, reader)?;
            }
            Ok(true)
        })?;

        // Report completion if not cancelled
        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: total,
                total,
                current_file: "Complete".to_string(),
                percent: 100,
            });
        }

        Ok(())
    }

    fn extract_directory(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} backend to extract directory '{}'",
            self.name(),
            dir_path
        );

        std::fs::create_dir_all(dest)?;

        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);

        let mut reader = ArchiveReader::open(path, pwd)?;

        let dir_prefix = format!("{}/", dir_path.trim_end_matches('/'));

        reader.for_each_entries(|entry, reader| {
            let entry_path = entry.name.clone();
            if entry_path.starts_with(&dir_prefix) || entry_path == dir_path {
                write_archive_entry(dest, entry, reader)?;
            }
            Ok(true)
        })?;

        Ok(())
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
            "Using {} backend unified extract (entries: {:?})",
            self.name(),
            entries.map(|e| e.len()).unwrap_or(0)
        );

        std::fs::create_dir_all(dest)?;

        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);
        let mut reader = ArchiveReader::open(archive, pwd)?;

        // First pass: count total entries to extract (for accurate progress)
        let archive_info = self.list(archive, password)?;
        let total_entries = match &entries {
            None => archive_info.entries.len(),
            Some(refs) => {
                // Count how many archive entries match our refs
                archive_info
                    .entries
                    .iter()
                    .filter(|e| refs.iter().any(|r| r.matches(&e.path)))
                    .count()
            }
        };

        let mut processed = 0usize;

        reader.for_each_entries(|entry, reader| {
            // Check for cancellation
            if let Some(token) = cancel {
                if token.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("Extraction cancelled");
                    return Err(sevenz_rust2::Error::Other(
                        "Extraction cancelled by user".into(),
                    ));
                }
            }

            let entry_path = entry.name.clone();

            // Check if this entry should be extracted
            let should_extract = match &entries {
                None => true, // Extract all
                Some(refs) => refs.iter().any(|r| r.matches(&entry_path)),
            };

            if should_extract {
                processed += 1;

                // Report progress
                if let Some(cb) = progress {
                    let percent = if total_entries > 0 {
                        ((processed * 100) / total_entries) as u8
                    } else {
                        0
                    };
                    cb(crate::ExtractionProgress {
                        current: processed,
                        total: total_entries,
                        current_file: entry_path.clone(),
                        percent,
                    });
                }

                write_archive_entry(dest, entry, reader)?;
            }
            Ok(true)
        })?;

        // Report completion
        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: total_entries,
                total: total_entries,
                current_file: "Complete".to_string(),
                percent: 100,
            });
        }

        Ok(())
    }

    fn recompress_7z(&self, source: &Path, dest_7z: &Path) -> Result<()> {
        use std::time::Instant;

        info!(
            "Using {} backend to recompress {} to {}",
            self.name(),
            source.display(),
            dest_7z.display()
        );

        // Use sevenz_rust2's compress functionality
        let start = Instant::now();
        info!("🔄 Starting compression with sevenz-rust2...");

        sevenz_rust2::compress_to_path(source, dest_7z)
            .context("Failed to recompress to 7z format")?;

        let elapsed = start.elapsed();
        info!("✅ Compression completed in {:.2}s", elapsed.as_secs_f64());

        Ok(())
    }

    fn add_files(&self, archive: &Path, files: &[PathBuf]) -> Result<()> {
        use std::time::Instant;

        info!(
            "Using {} backend to add {} files to archive",
            self.name(),
            files.len()
        );

        // 7z archives don't support in-place modification
        // We need to extract, add files, and recompress
        let total_start = Instant::now();
        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let extract_dir = temp_dir.path().join("extracted");

        // Extract existing archive
        let extract_start = Instant::now();
        info!("📂 Extracting existing archive...");
        self.extract_all(archive, &extract_dir, None)?;
        let extract_elapsed = extract_start.elapsed();
        info!(
            "✅ Extraction completed in {:.2}s",
            extract_elapsed.as_secs_f64()
        );

        // Copy new files to extracted directory
        for file in files {
            if !file.exists() {
                warn!("File does not exist, skipping: {}", file.display());
                continue;
            }

            let file_name = file
                .file_name()
                .ok_or_else(|| anyhow!("Invalid file path: {}", file.display()))?;
            let dest_path = extract_dir.join(file_name);

            std::fs::copy(file, &dest_path)
                .with_context(|| format!("Failed to copy {} to archive", file.display()))?;
        }

        // Recompress to original archive location
        let compress_start = Instant::now();
        info!("🔄 Recompressing modified archive...");
        let temp_archive = temp_dir.path().join("temp.7z");
        sevenz_rust2::compress_to_path(&extract_dir, &temp_archive)
            .context("Failed to create new archive")?;
        let compress_elapsed = compress_start.elapsed();
        info!(
            "✅ Recompression completed in {:.2}s",
            compress_elapsed.as_secs_f64()
        );

        // Replace original archive
        std::fs::copy(&temp_archive, archive).context("Failed to replace original archive")?;

        let total_elapsed = total_start.elapsed();
        info!(
            "📊 Total add_files time: {:.2}s (extract: {:.2}s, recompress: {:.2}s)",
            total_elapsed.as_secs_f64(),
            extract_elapsed.as_secs_f64(),
            compress_elapsed.as_secs_f64()
        );

        Ok(())
    }

    fn create_archive(&self, dest: &Path, files: &[PathBuf], _format: &str) -> Result<()> {
        use std::time::Instant;

        info!(
            "Using {} backend to create archive at {} with {} items",
            self.name(),
            dest.display(),
            files.len()
        );

        if files.is_empty() {
            return Err(anyhow!("No files provided to create archive"));
        }

        // Note: sevenz_rust2::compress_to_path compresses the CONTENTS of a directory,
        // not the directory itself. So we always need to use staging to preserve structure.

        // For multiple files/dirs, we need to stage them to compress together
        let start_staging = Instant::now();
        info!("📁 Creating staging directory and copying files...");

        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let staging_dir = temp_dir.path().join("staging");
        std::fs::create_dir_all(&staging_dir)?;

        // Copy files to staging directory
        for file in files {
            if !file.exists() {
                warn!("File does not exist, skipping: {}", file.display());
                continue;
            }

            let file_name = file
                .file_name()
                .ok_or_else(|| anyhow!("Invalid file path: {}", file.display()))?;
            let dest_path = staging_dir.join(file_name);

            if file.is_dir() {
                fs_extra::dir::copy(file, &staging_dir, &fs_extra::dir::CopyOptions::new())
                    .with_context(|| {
                        format!("Failed to copy directory {} to archive", file.display())
                    })?;
            } else {
                std::fs::copy(file, &dest_path)
                    .with_context(|| format!("Failed to copy {} to archive", file.display()))?;
            }
        }

        let staging_elapsed = start_staging.elapsed();
        info!(
            "✅ Staging completed in {:.2}s",
            staging_elapsed.as_secs_f64()
        );

        // Compress the staging directory
        let start_compress = Instant::now();
        info!("🔄 Starting compression with sevenz-rust2...");

        sevenz_rust2::compress_to_path(&staging_dir, dest)
            .context("Failed to create 7z archive")?;

        let compress_elapsed = start_compress.elapsed();
        info!(
            "✅ Compression completed in {:.2}s",
            compress_elapsed.as_secs_f64()
        );
        info!(
            "📊 Total time: {:.2}s (staging: {:.2}s, compression: {:.2}s)",
            (staging_elapsed + compress_elapsed).as_secs_f64(),
            staging_elapsed.as_secs_f64(),
            compress_elapsed.as_secs_f64()
        );

        Ok(())
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);

        let mut reader = ArchiveReader::open(archive, pwd)?;

        let mut content_bytes = Vec::new();
        let mut found = false;

        reader.for_each_entries(|entry, reader| {
            if entry.name == path_in_archive {
                std::io::copy(reader, &mut content_bytes)?;
                found = true;
                Ok(false) // Stop iteration
            } else {
                Ok(true)
            }
        })?;

        if !found {
            return Err(anyhow!("File not found in archive: {}", path_in_archive));
        }

        let content = String::from_utf8(content_bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).to_string());

        Ok(content)
    }

    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        info!(
            "Using {} backend to delete {} files from archive",
            self.name(),
            files.len()
        );

        // 7z archives don't support in-place modification
        // We need to extract, remove files, and recompress
        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let extract_dir = temp_dir.path().join("extracted");

        // Extract existing archive
        self.extract_all(archive, &extract_dir, None)?;

        // Delete specified files
        for file_path in files {
            let full_path = extract_dir.join(file_path);
            if full_path.exists() {
                if full_path.is_dir() {
                    std::fs::remove_dir_all(&full_path)
                        .with_context(|| format!("Failed to delete directory: {}", file_path))?;
                } else {
                    std::fs::remove_file(&full_path)
                        .with_context(|| format!("Failed to delete file: {}", file_path))?;
                }
            } else {
                warn!("File not found in archive, skipping: {}", file_path);
            }
        }

        // Recompress to original archive location
        let temp_archive = temp_dir.path().join("temp.7z");
        sevenz_rust2::compress_to_path(&extract_dir, &temp_archive)
            .context("Failed to create new archive")?;

        // Replace original archive
        std::fs::copy(&temp_archive, archive).context("Failed to replace original archive")?;

        Ok(())
    }

    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        info!("Adding/updating file '{}' in 7z archive", path_in_archive);

        // 7z archives don't support in-place modification
        // We need to extract, modify, and recompress
        let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let extract_dir = temp_dir.path().join("extracted");

        // Extract existing archive
        self.extract_all(archive, &extract_dir, None)?;

        // Write the new content
        let file_path = extract_dir.join(path_in_archive);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create parent directories")?;
        }

        let mut file = File::create(&file_path)
            .with_context(|| format!("Failed to create file: {}", path_in_archive))?;
        file.write_all(content.as_bytes())
            .context("Failed to write file content")?;

        // Recompress to original archive location
        let temp_archive = temp_dir.path().join("temp.7z");
        sevenz_rust2::compress_to_path(&extract_dir, &temp_archive)
            .context("Failed to create new archive")?;

        // Replace original archive
        std::fs::copy(&temp_archive, archive).context("Failed to replace original archive")?;

        Ok(())
    }

    fn convert_to_7z(&self, source: &crate::Archive, dest: &Path, temp_dir: &Path) -> Result<()> {
        use std::time::Instant;

        info!(
            "Converting {} to 7z format at {}",
            source.path().display(),
            dest.display()
        );

        let total_start = Instant::now();

        // First extract the source archive to temp directory using Archive handle (with password if needed)
        let extract_dir = temp_dir.join("extract");
        std::fs::create_dir_all(&extract_dir)?;

        // Extract using the Archive handle (which has password if needed)
        let extract_start = Instant::now();
        info!("📂 Extracting source archive to temp directory...");
        source
            .extract_all(&extract_dir)
            .context("Failed to extract source archive")?;
        let extract_elapsed = extract_start.elapsed();
        info!(
            "✅ Extraction completed in {:.2}s",
            extract_elapsed.as_secs_f64()
        );

        // Compress the extracted content to destination
        let compress_start = Instant::now();
        info!("🔄 Starting compression with sevenz-rust2 (this may take a while)...");
        info!("⚠️  NOTE: sevenz-rust2 compression is BLOCKING and may appear to hang");

        sevenz_rust2::compress_to_path(&extract_dir, dest)
            .context("Failed to create 7z archive")?;

        let compress_elapsed = compress_start.elapsed();
        info!(
            "✅ Compression completed in {:.2}s",
            compress_elapsed.as_secs_f64()
        );

        let total_elapsed = total_start.elapsed();
        info!(
            "📊 Total conversion time: {:.2}s (extract: {:.2}s, compress: {:.2}s)",
            total_elapsed.as_secs_f64(),
            extract_elapsed.as_secs_f64(),
            compress_elapsed.as_secs_f64()
        );

        Ok(())
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        let info = self.list(archive, password)?;
        for entry in info.entries {
            if entry.path == path_in_archive {
                return entry.crc32.ok_or_else(|| anyhow!("No CRC available"));
            }
        }
        Err(anyhow!("Entry not found"))
    }

    fn extract_entry_to_writer(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
        writer: &mut dyn std::io::Write,
    ) -> Result<()> {
        // NOTE: This method has a known issue with solid 7z archives.
        // When we return Ok(false) to stop iteration early after finding our target file,
        // the sevenz-rust2 library may throw ChecksumVerificationFailed errors because
        // solid blocks require full decompression for checksum validation.
        //
        // The FallbackBackend works around this by routing extract_entry_to_writer
        // directly to the CLI backend, which handles streaming correctly.
        // This method is kept for non-FallbackBackend usage scenarios.

        info!("Streaming entry '{}' from 7z archive", path_in_archive);
        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);
        let mut reader = ArchiveReader::open(archive, pwd)?;
        let mut found = false;

        reader.for_each_entries(|entry, reader| {
            if entry.name == path_in_archive {
                std::io::copy(reader, writer)?;
                found = true;
                Ok(false) // Stop iteration
            } else {
                Ok(true)
            }
        })?;

        if !found {
            return Err(anyhow!("File not found in archive: {}", path_in_archive));
        }
        Ok(())
    }
}

#[cfg(test)]
mod path_safety_tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{Duration, SystemTime};

    fn write_test_archive(archive: &Path, entry: sevenz_rust2::ArchiveEntry, contents: &[u8]) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(archive).unwrap();
        writer.set_encrypt_header(false);
        writer
            .push_archive_entry(entry, Some(Cursor::new(contents)))
            .unwrap();
        writer.finish().unwrap();
    }

    fn write_encrypted_test_archive(archive: &Path, entry_name: &str, contents: &[u8]) {
        use sevenz_rust2::encoder_options::AesEncoderOptions;

        let mut writer = sevenz_rust2::ArchiveWriter::create(archive).unwrap();
        writer.set_encrypt_header(false);
        writer.set_content_methods(vec![
            AesEncoderOptions::new(Password::from("correct-password")).into(),
            sevenz_rust2::EncoderMethod::LZMA2.into(),
        ]);
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(entry_name),
                Some(Cursor::new(contents)),
            )
            .unwrap();
        writer.finish().unwrap();
    }

    fn assert_time_close(actual: SystemTime, expected: SystemTime) {
        let drift = actual
            .duration_since(expected)
            .or_else(|_| expected.duration_since(actual))
            .unwrap();
        assert!(
            drift <= Duration::from_secs(3),
            "timestamp drifted by {drift:?}: actual={actual:?}, expected={expected:?}"
        );
    }

    #[test]
    fn native_entry_writer_rejects_parent_traversal_before_write() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let outside = temp.path().join("escaped.txt");
        let mut body = Cursor::new(b"owned".to_vec());
        let entry = sevenz_rust2::ArchiveEntry::new_file("../escaped.txt");

        let error = write_archive_entry(&dest, &entry, &mut body)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsafe") || error.contains("relative"));
        assert!(!outside.exists());
    }

    #[test]
    fn native_entry_writer_rejects_absolute_path_before_write() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let outside = temp.path().join("absolute-escaped.txt");
        let entry_name = outside.to_string_lossy();
        let mut body = Cursor::new(b"owned".to_vec());
        let entry = sevenz_rust2::ArchiveEntry::new_file(&entry_name);

        let error = write_archive_entry(&dest, &entry, &mut body)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsafe") || error.contains("relative"));
        assert!(!outside.exists());
    }

    #[test]
    fn native_entry_writer_writes_safe_nested_file() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let mut body = Cursor::new(b"safe".to_vec());
        let entry = sevenz_rust2::ArchiveEntry::new_file("Game/data.bin");

        write_archive_entry(&dest, &entry, &mut body).unwrap();

        assert_eq!(std::fs::read(dest.join("Game/data.bin")).unwrap(), b"safe");
    }

    #[test]
    fn native_entry_writer_does_not_clobber_existing_leaf() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let output = dest.join("existing.bin");
        std::fs::write(&output, b"original").unwrap();
        let mut body = Cursor::new(b"replacement".to_vec());
        let entry = sevenz_rust2::ArchiveEntry::new_file("existing.bin");

        write_archive_entry(&dest, &entry, &mut body).unwrap_err();

        assert_eq!(std::fs::read(output).unwrap(), b"original");
    }

    #[test]
    fn native_entry_writer_creates_directory_entries() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("dest");
        std::fs::create_dir(&dest).unwrap();
        let mut body = Cursor::new(Vec::new());
        let entry = sevenz_rust2::ArchiveEntry::new_directory("Game/data");

        write_archive_entry(&dest, &entry, &mut body).unwrap();

        assert!(dest.join("Game/data").is_dir());
    }

    #[test]
    fn native_extract_all_reports_maybe_bad_password() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("encrypted.7z");
        let dest = temp.path().join("dest");
        write_encrypted_test_archive(&archive, "secret.txt", b"classified contents");

        let error = SevenZBackend::new()
            .extract_all(&archive, &dest, Some("wrong-password"))
            .unwrap_err();

        assert!(
            error.chain().any(|cause| matches!(
                cause.downcast_ref::<sevenz_rust2::Error>(),
                Some(sevenz_rust2::Error::MaybeBadPassword(_))
            )),
            "unexpected wrong-password error: {error:#}"
        );
        assert!(!dest.join("secret.txt").exists());
    }

    #[test]
    fn native_extract_all_restores_file_timestamps() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("timestamped.7z");
        let dest = temp.path().join("dest");
        let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let archive_time = sevenz_rust2::NtTime::try_from(expected).unwrap();
        let mut entry = sevenz_rust2::ArchiveEntry::new_file("timestamped.txt");
        entry.has_access_date = true;
        entry.access_date = archive_time;
        entry.has_last_modified_date = true;
        entry.last_modified_date = archive_time;
        entry.has_creation_date = true;
        entry.creation_date = archive_time;
        write_test_archive(&archive, entry, b"timestamp contents");

        SevenZBackend::new()
            .extract_all(&archive, &dest, None)
            .unwrap();

        let metadata = std::fs::metadata(dest.join("timestamped.txt")).unwrap();
        assert_time_close(metadata.accessed().unwrap(), expected);
        assert_time_close(metadata.modified().unwrap(), expected);
        #[cfg(any(windows, target_os = "macos"))]
        assert_time_close(metadata.created().unwrap(), expected);
    }

    #[test]
    fn native_extract_all_rejects_malicious_archive_entry() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("malicious.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("escaped.txt");
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file("../escaped.txt"),
            b"owned",
        );

        SevenZBackend::new()
            .extract_all(&archive, &dest, None)
            .unwrap_err();

        assert!(!outside.exists());
    }

    #[test]
    fn native_extract_files_rejects_malicious_archive_entry() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("malicious.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("escaped.txt");
        let entry_name = "../escaped.txt";
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file(entry_name),
            b"owned",
        );

        SevenZBackend::new()
            .extract_files(&archive, &dest, &[entry_name.to_string()], None)
            .unwrap_err();

        assert!(!outside.exists());
    }

    #[test]
    fn native_extract_directory_rejects_malicious_archive_entry() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("malicious.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("escaped.txt");
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file("safe/../../escaped.txt"),
            b"owned",
        );

        SevenZBackend::new()
            .extract_directory(&archive, &dest, "safe", None)
            .unwrap_err();

        assert!(!outside.exists());
    }

    #[test]
    fn native_unified_extract_rejects_malicious_archive_entry() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("malicious.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("escaped.txt");
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file("../escaped.txt"),
            b"owned",
        );

        SevenZBackend::new()
            .extract(&archive, &dest, None, None, None, None)
            .unwrap_err();

        assert!(!outside.exists());
    }

    #[cfg(any(unix, windows))]
    fn create_directory_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_dir(target, link);

        match result {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping symlink assertion: {error}");
                false
            }
            Err(error) => panic!("create directory symlink: {error}"),
        }
    }

    #[cfg(any(unix, windows))]
    fn create_file_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(target, link);

        match result {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping symlink assertion: {error}");
                false
            }
            Err(error) => panic!("create file symlink: {error}"),
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_extract_all_rejects_static_symlink_parent() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("symlink-parent.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&dest).unwrap();
        std::fs::create_dir(&outside).unwrap();
        if !create_directory_symlink(&outside, &dest.join("linked")) {
            return;
        }
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file("linked/escaped.txt"),
            b"owned",
        );

        SevenZBackend::new()
            .extract_all(&archive, &dest, None)
            .unwrap_err();

        assert!(!outside.join("escaped.txt").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_extract_all_does_not_follow_final_leaf_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("symlink-leaf.7z");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("outside.txt");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&outside, b"original").unwrap();
        if !create_file_symlink(&outside, &dest.join("victim.txt")) {
            return;
        }
        write_test_archive(
            &archive,
            sevenz_rust2::ArchiveEntry::new_file("victim.txt"),
            b"replacement",
        );

        SevenZBackend::new()
            .extract_all(&archive, &dest, None)
            .unwrap_err();

        assert_eq!(std::fs::read(outside).unwrap(), b"original");
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{Duration, SystemTime};

    /// 2026-05-04 11:37:20 UTC, as whole seconds since the Unix epoch.
    const FIXTURE_UNIX_SECONDS: u64 = 1_777_894_640;

    fn write_archive_stamped_at(archive: &Path, entry_name: &str, unix_seconds: u64) {
        let stamp = sevenz_rust2::NtTime::try_from(
            SystemTime::UNIX_EPOCH + Duration::from_secs(unix_seconds),
        )
        .unwrap();
        let mut entry = sevenz_rust2::ArchiveEntry::new_file(entry_name);
        entry.has_last_modified_date = true;
        entry.last_modified_date = stamp;

        let mut writer = sevenz_rust2::ArchiveWriter::create(archive).unwrap();
        writer.set_encrypt_header(false);
        writer
            .push_archive_entry(entry, Some(Cursor::new(b"stamped contents".to_vec())))
            .unwrap();
        writer.finish().unwrap();
    }

    /// Opens a writer for `archive`; `password` switches the content
    /// coder chain to AES-256 + LZMA2, `encrypt_header` additionally
    /// encrypts the header itself (7-Zip's `-mhe=on`, "encrypt file
    /// names").
    fn writer_for(
        archive: &Path,
        password: Option<&str>,
        encrypt_header: bool,
    ) -> sevenz_rust2::ArchiveWriter<File> {
        use sevenz_rust2::encoder_options::AesEncoderOptions;

        let mut writer = sevenz_rust2::ArchiveWriter::create(archive).unwrap();
        writer.set_encrypt_header(encrypt_header);
        if let Some(password) = password {
            writer.set_content_methods(vec![
                AesEncoderOptions::new(Password::from(password)).into(),
                sevenz_rust2::EncoderMethod::LZMA2.into(),
            ]);
        }
        writer
    }

    fn push_file(writer: &mut sevenz_rust2::ArchiveWriter<File>, name: &str, contents: &[u8]) {
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(Cursor::new(contents.to_vec())),
            )
            .unwrap();
    }

    fn push_directory(writer: &mut sevenz_rust2::ArchiveWriter<File>, name: &str) {
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_directory(name),
                None::<Cursor<Vec<u8>>>,
            )
            .unwrap();
    }

    /// Records a zero-byte file the way 7-Zip records one: as an empty
    /// stream, with no block behind it. (Passing an empty *reader*
    /// instead would push a zero-length substream through the content
    /// coder chain -- a real, AES-coded block for an encrypted archive,
    /// which is not the shape this helper is for.)
    fn push_empty_file(writer: &mut sevenz_rust2::ArchiveWriter<File>, name: &str) {
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                None::<Cursor<Vec<u8>>>,
            )
            .unwrap();
    }

    fn entry<'info>(info: &'info ArchiveInfo, path: &str) -> &'info ArchiveEntry {
        info.entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("{path:?} is listed"))
    }

    /// The listed time must be the instant the archive actually records,
    /// rendered in the shape consumers parse `ArchiveEntry::modified`
    /// back out of -- not a derived approximation, and not an ISO form
    /// with a `T` separator and a `Z` suffix, which parses back to
    /// nothing and leaves every 7z entry dateless downstream.
    #[test]
    fn listed_entries_carry_the_instant_the_archive_records() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("stamped.7z");
        write_archive_stamped_at(&archive, "stamped.txt", FIXTURE_UNIX_SECONDS);

        let info = SevenZBackend::new()
            .list(&archive, None)
            .expect("the archive lists");

        let entry = info
            .entries
            .iter()
            .find(|entry| entry.path == "stamped.txt")
            .expect("the entry is listed");

        assert_eq!(
            entry.modified.as_deref(),
            Some("2026-05-04 11:37:20"),
            "a 7z entry must report the instant its header records, in \
             the shape every backend reports times in"
        );
    }

    /// An archive with no AES coder anywhere has nothing encrypted in
    /// it, and its listing must say so at both levels: no entry carries
    /// the flag, and the archive-level summary reports neither
    /// encryption nor a method. (Every stored file has a stream and a
    /// CRC -- neither is a signal of encryption.)
    #[test]
    fn a_plain_archives_entries_are_not_marked_encrypted() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("plain.7z");
        let mut writer = writer_for(&archive, None, false);
        push_file(&mut writer, "readme.txt", b"plain contents");
        push_directory(&mut writer, "sub");
        writer.finish().unwrap();

        let info = SevenZBackend::new()
            .list(&archive, None)
            .expect("the archive lists");

        assert!(
            !entry(&info, "readme.txt").encrypted,
            "a plain file entry must not be marked encrypted"
        );
        assert!(
            !entry(&info, "sub").encrypted,
            "a directory entry must not be marked encrypted"
        );
        assert!(
            !info.encrypted,
            "an archive with no encrypted entry must not be summarized as encrypted"
        );
        assert_eq!(info.encryption_method, None);
    }

    /// In an AES archive the flag is per entry, not per archive: a file
    /// whose stream decodes through the AES coder is encrypted, while a
    /// streamless entry -- a directory, an empty-stream file -- has no
    /// data for the cipher to cover and must not carry the flag.
    ///
    /// The header itself is left in the clear here (7-Zip's default
    /// without `-mhe=on`), which is why the structure lists without any
    /// password.
    #[test]
    fn an_aes_archives_file_entries_are_marked_encrypted_but_streamless_entries_are_not() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("aes.7z");
        let mut writer = writer_for(&archive, Some("correct-password"), false);
        push_file(&mut writer, "secret.txt", b"classified contents");
        push_empty_file(&mut writer, "empty.txt");
        push_directory(&mut writer, "sub");
        writer.finish().unwrap();

        let info = SevenZBackend::new()
            .list(&archive, None)
            .expect("a content-encrypted archive lists without its password");

        assert!(
            entry(&info, "secret.txt").encrypted,
            "an AES-coded file entry must be marked encrypted"
        );
        assert!(
            !entry(&info, "empty.txt").encrypted,
            "an empty file has no stream and must not be marked encrypted"
        );
        assert!(
            !entry(&info, "sub").encrypted,
            "a directory has no stream and must not be marked encrypted"
        );
        assert!(info.encrypted, "the archive-level summary must report it");
        assert_eq!(info.encryption_method.as_deref(), Some("7z"));
    }

    /// With the header encrypted too (`-mhe=on`), the listing itself
    /// needs the password: without one the open fails outright, and with
    /// it the entries decode as AES-coded and carry the flag.
    ///
    /// The fixture carries thirty entries rather than one because the
    /// writer's header encryption is best-effort: `finish` falls back to
    /// a *plain* header whenever the encrypted form would not undercut
    /// the raw one by at least 20 bytes, which a single-entry header
    /// always trips. Thirty long, repetitive names give it a header
    /// worth hiding, so the encrypted path genuinely engages.
    ///
    /// (`headers_encrypted` in the summary stays `false` even then --
    /// `sevenz_rust2::Archive` retains no trace of whether the header it
    /// decoded was encrypted, so this backend has nothing to report it
    /// from. The CLI tier does report it for the same archive.)
    #[test]
    fn a_header_encrypted_archive_lists_only_with_its_password_and_marks_entries() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("mhe.7z");
        let mut writer = writer_for(&archive, Some("correct-password"), true);
        for index in 0..30 {
            push_file(
                &mut writer,
                &format!("a-header-worth-hiding-from-passwordless-eyes/file-{index:02}.txt"),
                b"classified contents",
            );
        }
        writer.finish().unwrap();

        assert!(
            SevenZBackend::new().list(&archive, None).is_err(),
            "a header-encrypted archive must not list without its password"
        );

        let info = SevenZBackend::new()
            .list(&archive, Some("correct-password"))
            .expect("the password decrypts the header for listing");

        assert!(
            entry(
                &info,
                "a-header-worth-hiding-from-passwordless-eyes/file-00.txt"
            )
            .encrypted,
            "an AES-coded entry behind an encrypted header must carry the flag"
        );
        assert!(info.encrypted);
    }
}
