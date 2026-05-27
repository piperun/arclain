//! Archive operations

use super::HostFunctions;
use crate::types::PluginCapability;
use std::path::Path;
use tracing::info;

impl HostFunctions {
    pub(super) fn impl_current_archive_info(
        &mut self,
    ) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        // Resolve through the bridge so the answer always reflects
        // the currently active tab — see `crate::active_tab`.
        let archive = self.active_tab.as_ref()?.archive_path()?;
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
        let bridge = self
            .active_tab
            .as_ref()
            .ok_or("Active-tab bridge not configured")?;

        // Read the host's authoritative per-tab entries cache via
        // the bridge. The host populates this when `list_archive`
        // runs at open time, so we pay only an `Arc` clone + per-
        // entry `String` clone — never the multi-second cost of
        // re-listing through the archive backend (7z in particular
        // spawns a subprocess each call).
        //
        // An empty result with no archive_path means the plugin is
        // asking outside of an archive context — return the same
        // error the pre-bridge code did. Empty + an open archive
        // means the archive really has zero entries, or it's
        // encrypted and not yet unlocked; both produce an empty Vec
        // and the plugin's downstream "no codes detected" path
        // handles it gracefully.
        let entries = bridge.archive_entries();
        if entries.is_empty() && bridge.archive_path().is_none() {
            return Err("No archive currently open".to_string());
        }
        Ok(entries)
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

        let bridge = self
            .active_tab
            .as_ref()
            .ok_or("Active-tab bridge not configured")?;
        let current_path = bridge
            .archive_path()
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

        // Push the new path back through the bridge so the active
        // tab's `archive_path` signal reflects the rename — listeners
        // (status bar, breadcrumb, plugin re-reads) pick it up on
        // the next frame.
        let new_path_str = new_path.to_string_lossy().into_owned();
        bridge.set_archive_path(Some(new_path_str.clone()));

        info!(
            "[HostFunctions] Renamed archive from '{}' to '{}'",
            current_path, new_path_str
        );

        Ok(new_path_str)
    }
}
