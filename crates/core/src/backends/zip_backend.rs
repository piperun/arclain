//! ZIP archive backend using the `zip` crate
//!
//! Provides native Rust ZIP handling with full metadata support.

use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use zip::ZipArchive;

/// Native ZIP backend using the `zip` crate
#[derive(Clone)]
pub struct ZipBackend;

impl ZipBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ZipBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveBackend for ZipBackend {
    fn name(&self) -> &str {
        "Zip (Native)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::read_only()
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if ext == "zip" {
            Ok(ArchiveKind::Zip)
        } else {
            Err(anyhow!("Not a ZIP archive: {}", path.display()))
        }
    }

    fn list(&self, path: &Path, _password: Option<&str>) -> Result<ArchiveInfo> {
        info!("Using {} backend to list: {}", self.name(), path.display());

        let file = File::open(path).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

        let mut entries = Vec::with_capacity(archive.len());
        let mut has_encrypted = false;

        for i in 0..archive.len() {
            // Use raw access to get metadata without decryption
            match archive.by_index_raw(i) {
                Ok(file) => {
                    let is_encrypted = file.encrypted();
                    if is_encrypted {
                        has_encrypted = true;
                    }

                    let modified = file.last_modified().map(|dt| {
                        format!(
                            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                            dt.year(),
                            dt.month(),
                            dt.day(),
                            dt.hour(),
                            dt.minute(),
                            dt.second()
                        )
                    });

                    entries.push(ArchiveEntry {
                        path: file.name().to_string(),
                        size: file.size(),
                        packed_size: file.compressed_size(),
                        modified,
                        is_dir: file.is_dir(),
                        encrypted: is_encrypted,
                        crc32: Some(format!("{:08X}", file.crc32())),
                    });
                }
                Err(e) => {
                    warn!("Failed to read entry {}: {}", i, e);
                }
            }
        }

        debug!("Listed {} entries from ZIP archive", entries.len());

        Ok(ArchiveInfo {
            archive_path: path.to_path_buf(),
            archive_kind: ArchiveKind::Zip,
            entries,
            encrypted: has_encrypted,
            headers_encrypted: false,
            encryption_method: if has_encrypted {
                Some("ZipCrypto/AES".to_string())
            } else {
                None
            },
        })
    }

    fn extract_all(&self, path: &Path, dest: &Path, _password: Option<&str>) -> Result<()> {
        info!(
            "Using {} backend to extract {} to {}",
            self.name(),
            path.display(),
            dest.display()
        );

        let file = File::open(path).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

        std::fs::create_dir_all(dest).context("Failed to create destination directory")?;

        // Check if any files are encrypted
        for i in 0..archive.len() {
            if let Ok(entry) = archive.by_index_raw(i) {
                if entry.encrypted() {
                    return Err(anyhow!(
                        "ZIP contains encrypted files - use 7z CLI backend for password-protected ZIPs"
                    ));
                }
            }
        }

        // Use zip's built-in extract method for non-encrypted archives
        archive
            .extract(dest)
            .context("Failed to extract ZIP archive")?;

        Ok(())
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        _password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} backend to extract {} files",
            self.name(),
            files.len()
        );

        let file = File::open(path).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

        std::fs::create_dir_all(dest)?;

        for filename in files {
            match archive.by_name(filename) {
                Ok(mut zip_file) => {
                    if zip_file.encrypted() {
                        return Err(anyhow!("File is encrypted - use 7z CLI backend"));
                    }

                    let outpath = dest.join(zip_file.mangled_name());

                    if zip_file.is_dir() {
                        std::fs::create_dir_all(&outpath)?;
                    } else {
                        if let Some(parent) = outpath.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        let mut outfile = File::create(&outpath)?;
                        std::io::copy(&mut zip_file, &mut outfile)?;
                    }
                }
                Err(e) => {
                    warn!("File not found in ZIP: {} ({})", filename, e);
                }
            }
        }

        Ok(())
    }

    fn extract_directory(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        _password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} backend to extract directory '{}'",
            self.name(),
            dir_path
        );

        let file = File::open(path).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

        std::fs::create_dir_all(dest)?;

        // Normalize directory path
        let dir_prefix = if dir_path.ends_with('/') {
            dir_path.to_string()
        } else {
            format!("{}/", dir_path)
        };

        for i in 0..archive.len() {
            let mut zip_file = archive.by_index(i)?;
            let name = zip_file.name().to_string();

            if !name.starts_with(&dir_prefix) && name != dir_path {
                continue;
            }

            if zip_file.encrypted() {
                return Err(anyhow!(
                    "Directory contains encrypted files - use 7z CLI backend"
                ));
            }

            let outpath = dest.join(zip_file.mangled_name());

            if zip_file.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = File::create(&outpath)?;
                std::io::copy(&mut zip_file, &mut outfile)?;
            }
        }

        Ok(())
    }

    fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> Result<()> {
        Err(anyhow!("ZIP backend does not support creating 7z archives"))
    }

    fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> Result<()> {
        Err(anyhow!("ZIP backend is read-only"))
    }

    fn create_archive(&self, _dest: &Path, _files: &[PathBuf], _format: &str) -> Result<()> {
        Err(anyhow!("ZIP backend is read-only"))
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        _password: Option<&str>,
    ) -> Result<String> {
        let file = File::open(archive).context("Failed to open ZIP file")?;
        let mut archive = ZipArchive::new(file)?;

        let mut zip_file = archive.by_name(path_in_archive)?;

        if zip_file.encrypted() {
            return Err(anyhow!("File is encrypted - use 7z CLI backend"));
        }

        let mut content = String::new();
        zip_file.read_to_string(&mut content)?;
        Ok(content)
    }

    fn delete_files(&self, _archive: &Path, _files: &[String]) -> Result<()> {
        Err(anyhow!("ZIP backend is read-only"))
    }

    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _content: &str,
    ) -> Result<()> {
        Err(anyhow!("ZIP backend is read-only"))
    }

    fn convert_to_7z(&self, source: &crate::Archive, _dest: &Path, temp_dir: &Path) -> Result<()> {
        let extract_dir = temp_dir.join("zip_extract");
        source.extract_all(&extract_dir)?;

        Err(anyhow!(
            "ZIP backend extracted to {:?}, use another backend to create 7z",
            extract_dir
        ))
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        _password: Option<&str>,
    ) -> Result<String> {
        let file = File::open(archive)?;
        let mut zip = ZipArchive::new(file)?;

        // Use raw access to get CRC without decryption
        for i in 0..zip.len() {
            if let Ok(entry) = zip.by_index_raw(i) {
                if entry.name() == path_in_archive {
                    return Ok(format!("{:08X}", entry.crc32()));
                }
            }
        }

        Err(anyhow!("File not found in archive: {}", path_in_archive))
    }
}
