use crate::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

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
        info!(
            "Trying {} backend to list: {}",
            self.primary_name,
            path.display()
        );

        match self.primary.list(path, password) {
            Ok(info) => {
                info!(
                    "Successfully listed archive with {} backend",
                    self.primary_name
                );
                Ok(info)
            }
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
                        format!(
                            "Both {} and {} backends failed to extract",
                            self.primary_name, self.fallback_name
                        )
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
                    .with_context(|| format!("Both backends failed to extract files"))
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
                    .with_context(|| format!("Both backends failed to extract directory"))
            }
        }
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
                    .with_context(|| format!("Both backends failed to read file"))
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
                    .with_context(|| format!("Both backends failed to get CRC"))
            }
        }
    }
}
