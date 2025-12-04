use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use compress_tools::list_archive_files;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Libarchive-based backend supporting many archive formats (zip, tar, tar.gz, tar.bz2, etc.)
#[derive(Clone)]
pub struct LibarchiveBackend;

impl LibarchiveBackend {
    pub fn new() -> Self {
        Self
    }

    /// Detect archive kind from extension
    fn detect_kind(path: &Path) -> Result<ArchiveKind> {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Check for compound extensions first (tar.gz, tar.bz2, etc.)
        if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
            return Ok(ArchiveKind::Unknown("tar.gz".to_string()));
        }
        if filename.ends_with(".tar.bz2") || filename.ends_with(".tbz2") || filename.ends_with(".tbz") {
            return Ok(ArchiveKind::Unknown("tar.bz2".to_string()));
        }
        if filename.ends_with(".tar.xz") || filename.ends_with(".txz") {
            return Ok(ArchiveKind::Unknown("tar.xz".to_string()));
        }

        // Single extensions
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "zip" => Ok(ArchiveKind::Zip),
            "tar" => Ok(ArchiveKind::Unknown("tar".to_string())),
            "gz" | "gzip" => Ok(ArchiveKind::Unknown("gz".to_string())),
            "bz2" | "bzip2" => Ok(ArchiveKind::Unknown("bz2".to_string())),
            "xz" => Ok(ArchiveKind::Unknown("xz".to_string())),
            _ => Err(anyhow!("Unknown archive format: {}", path.display())),
        }
    }
}

impl ArchiveBackend for LibarchiveBackend {
    fn name(&self) -> &str {
        "Libarchive (Native)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Libarchive is primarily read-only for our purposes
        // Writing support varies by format and is complex
        BackendCapabilities::read_only()
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        Self::detect_kind(path)
    }

    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo> {
        info!("Using {} backend to list: {}", self.name(), path.display());

        if password.is_some() {
            warn!("Password provided but libarchive password support is limited");
        }

        let file = File::open(path).context("Failed to open archive file")?;
        let reader = BufReader::new(file);

        let files = list_archive_files(reader)
            .context("Failed to list archive contents with libarchive")?;

        let mut entries = Vec::new();
        for file_path in files {
            // compress-tools returns Vec<String>, not PathBuf
            let is_dir = file_path.ends_with('/');

            entries.push(ArchiveEntry {
                path: file_path,
                size: 0,           // Not available from list_archive_files
                packed_size: 0,    // Not available
                modified: None,    // Not available
                is_dir,
                encrypted: false,  // Can't easily detect
                crc32: None,       // Not available
            });
        }

        debug!("Listed {} entries from archive", entries.len());

        let archive_kind = Self::detect_kind(path)?;

        Ok(ArchiveInfo {
            archive_path: path.to_path_buf(),
            archive_kind,
            entries,
            encrypted: false,        // Can't easily detect with compress-tools
            headers_encrypted: false,
            encryption_method: None,
        })
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Using {} backend to extract {} to {}",
            self.name(),
            path.display(),
            dest.display()
        );

        if password.is_some() {
            warn!("Password provided but libarchive password support is limited");
        }

        std::fs::create_dir_all(dest).context("Failed to create destination directory")?;

        let file = File::open(path).context("Failed to open archive file")?;
        
        // compress-tools API: uncompress_archive(source, dest, ownership)
        compress_tools::uncompress_archive(&file, dest, compress_tools::Ownership::Preserve)
            .context("Failed to extract archive with libarchive")?;

        Ok(())
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} backend to extract {} files",
            self.name(),
            files.len()
        );

        if password.is_some() {
            warn!("Password provided but libarchive password support is limited");
        }

        // compress-tools doesn't support selective extraction directly
        // We'll extract to a temp dir and copy only the requested files
        let temp_dir = tempfile::tempdir()?;
        self.extract_all(path, temp_dir.path(), password)?;

        std::fs::create_dir_all(dest)?;

        // Copy requested files
        for file in files {
            let src = temp_dir.path().join(file);
            if src.exists() {
                let dst = dest.join(file);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if src.is_dir() {
                    // Copy directory recursively
                    copy_dir_all(&src, &dst)?;
                } else {
                    std::fs::copy(&src, &dst)?;
                }
            } else {
                warn!("Requested file not found in archive: {}", file);
            }
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

        if password.is_some() {
            warn!("Password provided but libarchive password support is limited");
        }

        // Extract to temp and copy the directory
        let temp_dir = tempfile::tempdir()?;
        self.extract_all(path, temp_dir.path(), password)?;

        std::fs::create_dir_all(dest)?;

        let src_dir = temp_dir.path().join(dir_path);
        if src_dir.exists() && src_dir.is_dir() {
            copy_dir_all(&src_dir, &dest.join(dir_path))?;
        } else {
            return Err(anyhow!("Directory not found in archive: {}", dir_path));
        }

        Ok(())
    }

    fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> Result<()> {
        Err(anyhow!("Libarchive backend does not support creating 7z archives"))
    }

    fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> Result<()> {
        Err(anyhow!("Libarchive backend is read-only, cannot modify archives"))
    }

    fn create_archive(&self, _dest: &Path, _files: &[PathBuf], _format: &str) -> Result<()> {
        Err(anyhow!("Libarchive backend is read-only, cannot create archives"))
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        let temp_dir = tempfile::tempdir()?;
        self.extract_files(
            archive,
            temp_dir.path(),
            &[path_in_archive.to_string()],
            password,
        )?;

        let extracted_file = temp_dir.path().join(path_in_archive);
        let content = std::fs::read_to_string(&extracted_file).or_else(|_| {
            // Try lossy UTF-8 if exact UTF-8 fails
            std::fs::read(&extracted_file)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        })?;

        Ok(content)
    }

    fn delete_files(&self, _archive: &Path, _files: &[String]) -> Result<()> {
        Err(anyhow!("Libarchive backend is read-only, cannot delete files"))
    }

    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _content: &str,
    ) -> Result<()> {
        Err(anyhow!("Libarchive backend is read-only, cannot modify files"))
    }

    fn convert_to_7z(&self, source: &Path, _dest: &Path, temp_dir: &Path) -> Result<()> {
        // Extract to temp, caller should use another backend to compress
        let extract_dir = temp_dir.join("libarchive_extract");
        self.extract_all(source, &extract_dir, None)?;

        Err(anyhow!(
            "Libarchive backend extracted to {:?}, use another backend to create 7z",
            extract_dir
        ))
    }

    fn crc32_of_entry(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _password: Option<&str>,
    ) -> Result<String> {
        Err(anyhow!("Libarchive backend does not provide CRC information"))
    }
}

/// Helper function to recursively copy a directory
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}