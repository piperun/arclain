pub mod sevenzip;
pub mod config;
pub mod logging;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use anyhow::Result;

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
        if !self.current_path.is_empty() {
            self.path_stack.push(self.current_path.clone());
        }
        self.current_path = if !self.current_path.is_empty() {
            format!("{}/{}", self.current_path, folder)
        } else {
            folder.to_string()
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
        let prefix = if self.current_path.is_empty() {
            String::new()
        } else {
            format!("{}/", self.current_path)
        };
        
        entries.iter()
            .filter_map(|e| {
                if self.current_path.is_empty() {
                    // Root level - show only top-level items
                    if !e.path.contains('/') && !e.path.contains('\\') {
                        Some(e.clone())
                    } else if let Some(pos) = e.path.find('/').or_else(|| e.path.find('\\')) {
                        // Show as folder
                        let folder = &e.path[..pos];
                        if !entries.iter().any(|other| other.path == folder && other.is_dir) {
                            Some(ArchiveEntry {
                                path: folder.to_string(),
                                size: 0,
                                packed_size: 0,
                                modified: None,
                                is_dir: true,
                                encrypted: false,
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else if e.path.starts_with(&prefix) {
                    // Inside a folder - show items in this folder
                    let relative = &e.path[prefix.len()..];
                    if !relative.contains('/') && !relative.contains('\\') {
                        Some(ArchiveEntry {
                            path: relative.to_string(),
                            size: e.size,
                            packed_size: e.packed_size,
                            modified: e.modified.clone(),
                            is_dir: e.is_dir,
                            encrypted: e.encrypted,
                        })
                    } else if let Some(pos) = relative.find('/').or_else(|| relative.find('\\')) {
                        let folder = &relative[..pos];
                        Some(ArchiveEntry {
                            path: folder.to_string(),
                            size: 0,
                            packed_size: 0,
                            modified: None,
                            is_dir: true,
                            encrypted: false,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut acc, e| {
                // Deduplicate folders
                if !acc.iter().any(|existing| existing.path == e.path) {
                    acc.push(e);
                }
                acc
            })
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub archive_path: PathBuf,
    pub archive_kind: ArchiveKind,
    pub entries: Vec<ArchiveEntry>,
}

pub trait ArchiveBackend: Send + Sync {
    fn identify(&self, path: &Path) -> Result<ArchiveKind>;
    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo>;
    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()>;
    fn extract_files(&self, path: &Path, dest: &Path, files: &[String], password: Option<&str>) -> Result<()>;
    fn recompress_7z(&self, source: &Path, dest_7z: &Path) -> Result<()>;
    fn add_files(&self, archive: &Path, files: &[PathBuf]) -> Result<()>;
    fn create_archive(&self, dest: &Path, files: &[PathBuf], format: &str) -> Result<()>;
}

pub use config::{Config, PassRule, ConfigStore};


