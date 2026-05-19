use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Strategy for determining which files to extract when opening a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenStrategy {
    /// Extract only the selected file
    FileOnly,
    /// Extract the entire directory containing the file
    SameDirectory,
    /// Extract file and analyze dependencies (Windows PE, ELF, etc.)
    WithDependencies,
}

/// Find the full path of a file in the archive entries by matching filename
fn find_full_path_in_entries(target_file: &str, all_entries: &[String]) -> Option<String> {
    let target_normalized = target_file.replace('\\', "/");
    let target_filename = target_normalized
        .rsplit('/')
        .next()
        .unwrap_or(&target_normalized);

    // First try exact match
    if all_entries
        .iter()
        .any(|e| e.replace('\\', "/") == target_normalized)
    {
        return Some(target_file.to_string());
    }

    // Try to find by filename (case-insensitive)
    for entry in all_entries {
        let entry_normalized = entry.replace('\\', "/");
        let entry_filename = entry_normalized
            .rsplit('/')
            .next()
            .unwrap_or(&entry_normalized);

        if entry_filename.eq_ignore_ascii_case(target_filename) {
            return Some(entry.clone());
        }
    }

    None
}

/// Handles opening files from archives by extracting necessary files to temp
pub struct FileOpener {
    temp_dir: PathBuf,
}

impl FileOpener {
    /// Create a new FileOpener with a temporary directory
    pub fn new() -> Result<Self> {
        let temp_dir = std::env::temp_dir().join(format!("arclain_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;
        info!("Created temporary directory: {}", temp_dir.display());

        Ok(Self { temp_dir })
    }

    /// Get the list of files to extract based on the strategy
    pub fn get_files_to_extract(
        &self,
        target_file: &str,
        all_entries: &[String],
        strategy: OpenStrategy,
    ) -> Vec<String> {
        // First, find the actual full path in all_entries
        // target_file might be just the filename or a partial path
        let full_path = find_full_path_in_entries(target_file, all_entries);
        let target = full_path.as_deref().unwrap_or(target_file);

        debug!("Resolved target path: {} -> {}", target_file, target);

        match strategy {
            OpenStrategy::FileOnly => {
                vec![target.to_string()]
            }
            OpenStrategy::SameDirectory => self.get_same_directory_files(target, all_entries),
            OpenStrategy::WithDependencies => self.get_files_with_dependencies(target, all_entries),
        }
    }

    /// Get all files in the same directory as the target file
    fn get_same_directory_files(&self, target_file: &str, all_entries: &[String]) -> Vec<String> {
        // Normalize path separators
        let target = target_file.replace('\\', "/");

        // Get the directory of the target file
        let dir = if let Some(idx) = target.rfind('/') {
            &target[..idx]
        } else {
            "" // Root directory
        };

        debug!(
            "Extracting all files from directory: {}",
            if dir.is_empty() { "<root>" } else { dir }
        );

        // Filter entries that are in the same directory
        all_entries
            .iter()
            .filter(|entry| {
                let entry_normalized = entry.replace('\\', "/");

                // Get the directory of this entry
                let entry_dir = if let Some(idx) = entry_normalized.rfind('/') {
                    &entry_normalized[..idx]
                } else {
                    ""
                };

                // Include if in the same directory or any subdirectory
                if dir.is_empty() {
                    true
                } else {
                    entry_dir == dir || entry_dir.starts_with(&format!("{}/", dir))
                }
            })
            .cloned()
            .collect()
    }

    /// Get files with dependencies (advanced strategy)
    fn get_files_with_dependencies(
        &self,
        target_file: &str,
        all_entries: &[String],
    ) -> Vec<String> {
        // Start with same directory files
        let mut files = self.get_same_directory_files(target_file, all_entries);

        // Determine file type and look for common dependencies
        let ext = target_file.rsplit('.').next().unwrap_or("").to_lowercase();

        match ext.as_str() {
            "exe" | "dll" => {
                // For Windows executables, include common DLL patterns
                debug!("Target is Windows executable, looking for DLL dependencies");
                self.add_dll_dependencies(&mut files, all_entries);
            }
            // Add more file type handlers here
            _ => {}
        }

        files
    }

    /// Add common DLL dependencies for Windows executables
    fn add_dll_dependencies(&self, files: &mut Vec<String>, all_entries: &[String]) {
        let mut seen: HashSet<String> = files.iter().cloned().collect();
        for entry in all_entries {
            let lower = entry.to_lowercase();
            if (lower.ends_with(".dll") || lower.ends_with(".config"))
                && seen.insert(entry.clone())
            {
                debug!("Adding potential dependency: {}", entry);
                files.push(entry.clone());
            }
        }
    }

    /// Open a file from the temporary directory
    pub fn open_extracted_file(&self, relative_path: &str) -> Result<()> {
        let file_path = self.temp_dir.join(relative_path);

        if !file_path.exists() {
            anyhow::bail!("File not found in temp directory: {}", file_path.display());
        }

        info!("Opening file with system handler: {}", file_path.display());

        #[cfg(target_os = "windows")]
        {
            // Use explorer.exe for reliable file opening - handles all path types
            let status = Command::new("explorer")
                .arg(&file_path)
                .spawn()
                .context("Failed to spawn explorer")?;
            info!("Launched explorer with PID: {}", status.id());
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(&file_path)
                .spawn()
                .context("Failed to open file")?;
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("xdg-open")
                .arg(&file_path)
                .spawn()
                .context("Failed to open file")?;
        }

        Ok(())
    }

    /// Get the temporary directory path
    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    /// Clean up the temporary directory
    pub fn cleanup(&self) -> Result<()> {
        if self.temp_dir.exists() {
            info!(
                "Cleaning up temporary directory: {}",
                self.temp_dir.display()
            );
            std::fs::remove_dir_all(&self.temp_dir).context("Failed to remove temp directory")?;
        }
        Ok(())
    }
}

impl Drop for FileOpener {
    fn drop(&mut self) {
        // Attempt cleanup on drop, but don't panic if it fails
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests;
