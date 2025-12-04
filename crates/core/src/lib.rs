pub mod archive;
pub mod archive_organizer;
pub mod backends;
pub mod config;
pub mod config_db;
pub mod file_opener;
pub mod logging;
pub mod organization;
pub mod sevenzip;
pub mod title_filter;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// Add this to lib.rs
#[derive(Debug, Clone)]
pub struct NavigationState {
    pub current_path: String,
    pub path_stack: Vec<String>,
    pub forward_stack: Vec<String>,
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            current_path: String::new(),
            path_stack: vec![],
            forward_stack: vec![],
        }
    }

    pub fn navigate_to(&mut self, folder: &str) {
        let segment = Self::normalize_path(folder);
        if segment.is_empty() {
            return;
        }

        let current = Self::normalize_path(&self.current_path);
        if !current.is_empty() {
            self.path_stack.push(current.clone());
        }

        self.current_path = if current.is_empty() {
            segment
        } else {
            format!("{}/{}", current, segment)
        };
        // Clear forward stack when navigating to a new location
        self.forward_stack.clear();
    }

    pub fn navigate_back(&mut self) -> bool {
        if let Some(prev) = self.path_stack.pop() {
            self.forward_stack.push(self.current_path.clone());
            self.current_path = prev;
            true
        } else if !self.current_path.is_empty() {
            self.forward_stack.push(self.current_path.clone());
            self.current_path.clear();
            true
        } else {
            false
        }
    }

    pub fn navigate_forward(&mut self) -> bool {
        if let Some(next) = self.forward_stack.pop() {
            self.path_stack.push(self.current_path.clone());
            self.current_path = next;
            true
        } else {
            false
        }
    }

    pub fn navigate_up(&mut self) -> bool {
        if self.current_path.is_empty() {
            return false;
        }

        // Find the last separator and go to parent
        if let Some(pos) = self.current_path.rfind('/') {
            self.path_stack.push(self.current_path.clone());
            self.current_path = self.current_path[..pos].to_string();
            self.forward_stack.clear();
            true
        } else {
            // We're at a top-level folder, go to root
            self.path_stack.push(self.current_path.clone());
            self.current_path.clear();
            self.forward_stack.clear();
            true
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.path_stack.is_empty() || !self.current_path.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    pub fn can_go_up(&self) -> bool {
        !self.current_path.is_empty()
    }

    pub fn set_current_path(&mut self, path: &str) {
        self.current_path = Self::normalize_path(path);
    }

    pub fn get_all_folders(&self, entries: &[ArchiveEntry]) -> Vec<String> {
        let mut folders = std::collections::HashSet::new();

        for entry in entries {
            // Normalize path separators to forward slashes
            let normalized_path = entry.path.replace('\\', "/");

            if entry.is_dir {
                folders.insert(normalized_path.clone());
            }

            // Extract folder paths from file paths
            let mut path = normalized_path;
            while let Some(pos) = path.rfind('/') {
                path = path[..pos].to_string();
                if !path.is_empty() {
                    folders.insert(path.clone());
                }
            }
        }

        let mut folder_vec: Vec<String> = folders.into_iter().collect();
        folder_vec.sort();
        folder_vec
    }

    pub fn filter_entries(&self, entries: &[ArchiveEntry]) -> Vec<ArchiveEntry> {
        let normalized_current = self.current_path.replace('\\', "/");
        let prefix = if normalized_current.is_empty() {
            String::new()
        } else {
            format!("{}/", normalized_current)
        };

        let items: Vec<ArchiveEntry> = entries
            .iter()
            .filter_map(|e| {
                let normalized_path = e.path.replace('\\', "/");

                if self.current_path.is_empty() {
                    if !normalized_path.contains('/') {
                        let mut entry = e.clone();
                        entry.path = normalized_path;
                        return Some(entry);
                    }

                    if let Some(pos) = normalized_path.find('/') {
                        let folder = normalized_path[..pos].to_string();
                        return Some(ArchiveEntry {
                            path: folder,
                            size: 0,
                            packed_size: 0,
                            modified: None,
                            is_dir: true,
                            encrypted: false,
                            crc32: None,
                        });
                    }

                    None
                } else if normalized_path.starts_with(&prefix) {
                    let relative = &normalized_path[prefix.len()..];
                    if relative.is_empty() {
                        return None;
                    }

                    if !relative.contains('/') {
                        let mut entry = e.clone();
                        entry.path = relative.to_string();
                        return Some(entry);
                    }

                    if let Some(pos) = relative.find('/') {
                        let folder = relative[..pos].to_string();
                        return Some(ArchiveEntry {
                            path: folder,
                            size: 0,
                            packed_size: 0,
                            modified: None,
                            is_dir: true,
                            encrypted: false,
                            crc32: None,
                        });
                    }

                    None
                } else {
                    None
                }
            })
            .collect();

        use std::collections::BTreeMap;

        let mut map: BTreeMap<String, ArchiveEntry> = BTreeMap::new();
        for entry in items {
            map.entry(entry.path.clone())
                .and_modify(|existing| {
                    if existing.modified.is_none() && entry.modified.is_some() {
                        *existing = entry.clone();
                    }
                })
                .or_insert(entry);
        }

        let mut result: Vec<ArchiveEntry> = map.into_values().collect();

        // Update folder sizes and CRC
        for entry in result.iter_mut().filter(|e| e.is_dir) {
            let full_path = if normalized_current.is_empty() {
                entry.path.clone()
            } else {
                format!("{}/{}", normalized_current, entry.path)
            };

            let (size, packed) = Self::compute_folder_totals(entries, &full_path);
            entry.size = size;
            entry.packed_size = packed;

            // Compute aggregated CRC-32 over descendant file CRCs
            entry.crc32 = Self::compute_folder_crc(entries, &full_path);
        }

        result
    }
}

impl NavigationState {
    fn compute_folder_totals(entries: &[ArchiveEntry], folder_path: &str) -> (u64, u64) {
        let normalized_folder = Self::normalize_path(folder_path);
        let prefix = format!("{}/", normalized_folder.trim_end_matches('/'));
        let mut size = 0u64;
        let mut packed = 0u64;

        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let normalized = entry.path.replace('\\', "/");
            if normalized == normalized_folder || normalized.starts_with(&prefix) {
                size = size.saturating_add(entry.size);
                packed = packed.saturating_add(entry.packed_size);
            }
        }

        (size, packed)
    }

    fn compute_folder_crc(entries: &[ArchiveEntry], folder_path: &str) -> Option<String> {
        use crc32fast::Hasher;
        let normalized_folder = Self::normalize_path(folder_path);
        let prefix = format!("{}/", normalized_folder.trim_end_matches('/'));
        let mut items: Vec<(String, String)> = Vec::new();

        for entry in entries {
            if entry.is_dir {
                continue;
            }
            let normalized = entry.path.replace('\\', "/");
            if normalized == normalized_folder || normalized.starts_with(&prefix) {
                if let Some(crc) = &entry.crc32 {
                    items.push((normalized.clone(), crc.to_uppercase()));
                }
            }
        }

        if items.is_empty() {
            return None;
        }

        items.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Hasher::new();
        for (p, c) in items {
            hasher.update(p.as_bytes());
            hasher.update(b":");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }
        let sum = hasher.finalize();
        Some(format!("{:08X}", sum))
    }

    fn normalize_path(path: &str) -> String {
        path.split(|c| c == '/' || c == '\\')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveKind {
    Zip,
    SevenZ,
    Rar,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub size: u64,
    pub packed_size: u64,
    pub modified: Option<String>,
    pub is_dir: bool,
    pub encrypted: bool,
    pub crc32: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub archive_path: PathBuf,
    pub archive_kind: ArchiveKind,
    pub entries: Vec<ArchiveEntry>,
    pub encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
}

/// Capabilities that an archive backend may support
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Can extract files from archives
    pub can_extract: bool,
    /// Can create new archives
    pub can_create: bool,
    /// Can add files to existing archives
    pub can_add_files: bool,
    /// Can delete files from archives
    pub can_delete_files: bool,
    /// Can modify files in archives
    pub can_modify_files: bool,
    /// Can recompress to 7z format
    pub can_recompress_7z: bool,
    /// Can convert to 7z format
    pub can_convert_to_7z: bool,
}

impl BackendCapabilities {
    /// Creates capabilities for a read-only backend
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

    /// Creates capabilities for a full-featured backend
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

    /// Check if the backend is read-only (no write operations)
    pub fn is_read_only(&self) -> bool {
        !self.can_create
            && !self.can_add_files
            && !self.can_delete_files
            && !self.can_modify_files
    }
}

pub trait ArchiveBackend: Send + Sync {
    /// Returns the name of this backend for logging/display purposes
    fn name(&self) -> &str;

    /// Returns the capabilities of this backend
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

    // New capabilities for inline editing
    /// Read a single file from the archive as UTF-8 text. Lossy-decoding for non-UTF8.
    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String>;

    /// Delete specific files from the archive.
    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()>;

    /// Add or replace a file in the archive from in-memory text content.
    /// The file will be stored at `path_in_archive`.
    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()>;

    /// Convert an archive to 7z format using a temporary directory.
    fn convert_to_7z(&self, source: &Path, dest: &Path, temp_dir: &Path) -> Result<()>;

    /// Compute CRC-32 of a specific entry (useful for encrypted files where listing doesn't provide it).
    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String>;
}

pub use archive::Archive;
pub use config::{Config, ConfigStore, PassRule};
pub use config_db::{open_databases, ConfigDb, ConfigDbs, DbPaths, SecretsDb, SecretsKey};
