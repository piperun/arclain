//! Archive operations

use super::HostFunctions;
use crate::types::PluginCapability;
use std::path::Path;
use tracing::info;

impl HostFunctions {
    pub(super) fn impl_current_archive_info(
        &mut self,
    ) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        // Return current archive path if available
        let archive: String = self.current_archive.lock().clone()?;
        let path_buf = std::path::PathBuf::from(&archive);
        let filename: String = path_buf.file_name()?.to_str()?.to_string();

        Some(crate::arclain::plugin::host::ArchiveInfo {
            path: archive,
            filename,
        })
    }

    pub(super) fn impl_list_archive_files(&mut self) -> std::result::Result<Vec<String>, String> {
        if !self.check_capability(PluginCapability::ArchiveMetadataRead) {
            return Err("ArchiveMetadataRead capability not granted".to_string());
        }
        let backend = self
            .archive_backend
            .as_ref()
            .ok_or("Archive backend not available")?;
        let archive = self
            .current_archive
            .lock()
            .clone()
            .ok_or("No archive currently open")?;
        let password = self.current_password.lock().clone();

        let info = backend
            .list(Path::new(&archive), password.as_deref())
            .map_err(|e| e.to_string())?;
        Ok(info.entries.into_iter().map(|e| e.path).collect())
    }

    /// Rename the currently open archive file
    pub(super) fn impl_rename_archive(
        &mut self,
        new_name: String,
    ) -> std::result::Result<String, String> {
        // Require ArchiveModify capability
        if !self.check_capability(PluginCapability::ArchiveModify) {
            return Err("ArchiveModify capability not granted".to_string());
        }

        let current_path = self
            .current_archive
            .lock()
            .clone()
            .ok_or("No archive currently open")?;

        let path = Path::new(&current_path);
        let parent = path.parent().ok_or("Cannot determine parent directory")?;

        // Sanitize the new name to prevent path traversal
        let safe_name = new_name
            .replace(['/', '\\'], "_")
            .trim()
            .to_string();

        if safe_name.is_empty() {
            return Err("New name cannot be empty".to_string());
        }

        let new_path = parent.join(&safe_name);

        // Check if target already exists
        if new_path.exists() && new_path != path {
            return Err(format!(
                "A file named '{}' already exists",
                safe_name
            ));
        }

        // Perform the rename
        std::fs::rename(&path, &new_path).map_err(|e| format!("Failed to rename: {}", e))?;

        // Update the current archive path
        let new_path_str = new_path.to_string_lossy().to_string();
        *self.current_archive.lock() = Some(new_path_str.clone());

        info!(
            "[HostFunctions] Renamed archive from '{}' to '{}'",
            current_path, new_path_str
        );

        Ok(new_path_str)
    }
}
