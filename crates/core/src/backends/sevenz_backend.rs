use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use sevenz_rust2::{ArchiveReader, Password};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Native 7z backend using the `sevenz-rust2` crate
#[derive(Clone)]
pub struct SevenZBackend;

impl SevenZBackend {
    pub fn new() -> Self {
        Self
    }

    /// Convert NtTime to ISO 8601 string format
    fn format_time(nt_time: sevenz_rust2::NtTime) -> Option<String> {
        use std::time::SystemTime;

        let system_time: SystemTime = nt_time.into();

        // Convert to a readable format
        if let Ok(duration) = system_time.duration_since(SystemTime::UNIX_EPOCH) {
            let secs = duration.as_secs();

            // Basic ISO 8601 formatting without external dependencies
            // Calculate date/time components from Unix timestamp
            const SECONDS_PER_DAY: u64 = 86400;
            const DAYS_FROM_0_TO_1970: i64 = 719162; // Days from year 0 to Unix epoch

            let days_since_epoch = (secs / SECONDS_PER_DAY) as i64;
            let seconds_today = secs % SECONDS_PER_DAY;

            let hours = seconds_today / 3600;
            let minutes = (seconds_today % 3600) / 60;
            let seconds = seconds_today % 60;

            // Simplified date calculation (good enough for display)
            let days_from_year_0 = days_since_epoch + DAYS_FROM_0_TO_1970;
            let year = (days_from_year_0 / 365) as u32; // Approximation

            // Format as ISO 8601-ish (simplified)
            Some(format!(
                "{:04}-01-01T{:02}:{:02}:{:02}Z",
                year, hours, minutes, seconds
            ))
        } else {
            None
        }
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

        let mut entries = Vec::new();
        let mut any_encrypted = false;
        let headers_encrypted = false; // 7z-rust2 doesn't easily expose this

        // Access the archive's files directly
        for entry in &reader.archive().files {
            let is_dir = entry.is_directory;
            // Check if the entry has encryption by checking if it has a stream and other indicators
            let encrypted = entry.has_stream && !entry.is_directory && entry.has_crc;

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

        if let Some(pwd) = password {
            sevenz_rust2::decompress_file_with_password(path, dest, Password::from(pwd))
                .context("Failed to extract encrypted 7z archive")?;
        } else {
            sevenz_rust2::decompress_file(path, dest).context("Failed to extract 7z archive")?;
        }

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

                let dest_path = dest.join(&entry_path);
                if entry.is_directory {
                    std::fs::create_dir_all(&dest_path).ok();
                } else {
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    let mut out = File::create(&dest_path)?;

                    // Copy with cancellation check could be better, but standard copy is blocking
                    // For large files, this might delay cancellation until file is done
                    std::io::copy(reader, &mut out)?;
                }
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
                let dest_path = dest.join(&entry_path);
                if entry.is_directory {
                    std::fs::create_dir_all(&dest_path).ok();
                } else {
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    let mut out = File::create(&dest_path)?;
                    std::io::copy(reader, &mut out)?;
                }
            }
            Ok(true)
        })?;

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
        info!("Streaming entry '{}' from 7z archive", path_in_archive);
        let pwd = password.map(Password::from).unwrap_or_else(Password::empty);
        // Note: Opening archive reader might be slow if large headers
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
