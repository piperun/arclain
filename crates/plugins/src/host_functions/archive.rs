//! Archive operations

use super::HostFunctions;
use crate::types::PluginCapability;
use arclain_core::utilities::rename_no_replace;
use std::io;
use std::path::Path;
use tracing::{error, info};

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;

impl HostFunctions {
    pub(super) fn impl_current_archive_info(
        &mut self,
    ) -> Option<crate::arclain::plugin::host::ArchiveInfo> {
        // Prefer the per-event context: if the dispatch worker
        // installed one, the handler is running for a specific
        // event's archive, not necessarily the currently active
        // tab. Falls back to the bridge for non-event paths (panel
        // render, UI actions outside event dispatch).
        let archive = if let Some(ref ctx) = self.event_context {
            ctx.archive_path.clone()
        } else {
            self.active_tab.as_ref()?.archive_path()?
        };
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

        // Per-event context wins (see `impl_current_archive_info`).
        // The event payload carries the originating tab's entries
        // snapshotted at fire time, so a plugin handler processing
        // a queued event sees that tab's files even when the user
        // has switched tabs in the meantime.
        if let Some(ref ctx) = self.event_context {
            return Ok(ctx.entries.iter().map(|e| e.path.clone()).collect());
        }

        // Non-event path (panel render, etc.): read the host's
        // authoritative per-tab entries cache via the bridge —
        // populated by `list_archive` at open time so we pay only
        // an `Arc` clone + per-entry `String` clone.
        let bridge = self
            .active_tab
            .as_ref()
            .ok_or("Active-tab bridge not configured")?;
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
        let current_path = bridge.archive_path().ok_or("No archive currently open")?;

        let path = Path::new(&current_path);
        let parent = path.parent().ok_or("Cannot determine parent directory")?;

        // Sanitize the new name to prevent path traversal
        let safe_name = new_name.replace(['/', '\\'], "_").trim().to_string();

        if safe_name.is_empty() {
            return Err("New name cannot be empty".to_string());
        }

        let new_path = parent.join(&safe_name);

        if new_path == path {
            return Ok(current_path);
        }

        // `exists()` followed by `rename()` is racy, and `std::fs::rename`
        // replaces an existing file on supported desktop platforms. Use the
        // platform's atomic no-replace rename primitive instead.
        if let Err(rename_error) = rename_no_replace(path, &new_path) {
            error!(
                error = %rename_error,
                source = %path.display(),
                destination = %new_path.display(),
                "Failed to rename archive without replacing the destination"
            );
            return if rename_error.kind() == io::ErrorKind::AlreadyExists {
                Err(format!("A file named '{}' already exists", safe_name))
            } else {
                Err("Failed to rename archive safely".to_string())
            };
        }

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
