//! Archive operations and management
//! 
//! This module provides the core archive abstraction layer, including:
//! - Archive handle with dependency injection
//! - ArchiveBackend trait for format implementations  
//! - Archive metadata types
//! - Navigation state for UI

pub mod handle;
pub mod info;
pub mod navigation;

use anyhow::Result;
use std::path::{Path, PathBuf};

pub use handle::Archive;
pub use info::{ArchiveEntry, ArchiveInfo, ArchiveKind};
pub use navigation::NavigationState;

/// Capabilities that an archive backend may support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub can_extract: bool,
    pub can_create: bool,
    pub can_add_files: bool,
    pub can_delete_files: bool,
    pub can_modify_files: bool,
    pub can_recompress_7z: bool,
    pub can_convert_to_7z: bool,
}

impl BackendCapabilities {
    pub const fn read_only() -> Self {
        Self {
            can_extract: true,
            can_create: false,
            can_add_files: false,
            can_delete_files: false,
            can_modify_files: false,
            can_recompress_7z: false,
            can_convert_to_7z: false,
        }
    }

    pub const fn full_featured() -> Self {
        Self {
            can_extract: true,
            can_create: true,
            can_add_files: true,
            can_delete_files: true,
            can_modify_files: true,
            can_recompress_7z: true,
            can_convert_to_7z: true,
        }
    }

    pub fn is_read_only(&self) -> bool {
        !self.can_create
            && !self.can_add_files
            && !self.can_delete_files
            && !self.can_modify_files
    }
}

/// Trait for archive backend implementations
pub trait ArchiveBackend: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> BackendCapabilities;
    fn identify(&self, path: &Path) -> Result<ArchiveKind>;
    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo>;
    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()>;
    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()>;
    fn extract_directory(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<()>;
    fn recompress_7z(&self, source: &Path, dest_7z: &Path) -> Result<()>;
    fn add_files(&self, archive: &Path, files: &[PathBuf]) -> Result<()>;
    fn create_archive(&self, dest: &Path, files: &[PathBuf], format: &str) -> Result<()>;
    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String>;
    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()>;
    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()>;
    fn convert_to_7z(&self, source: &Archive, dest: &Path, temp_dir: &Path) -> Result<()>;
    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String>;
}