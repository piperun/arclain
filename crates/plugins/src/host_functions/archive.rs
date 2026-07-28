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

pub(super) const MAX_ARCHIVE_PAGE_ITEMS: usize = 256;
const MAX_ARCHIVE_PAGE_BYTES: usize = 1024 * 1024;

pub(super) fn archive_entry_count(entries: &[arclain_core::ArchiveEntry]) -> u64 {
    u64::try_from(entries.len()).unwrap_or(u64::MAX)
}

pub(super) fn archive_entry_page(
    entries: &[arclain_core::ArchiveEntry],
    offset: u32,
    limit: u32,
) -> std::result::Result<Vec<String>, String> {
    let limit = usize::try_from(limit).map_err(|_| "archive page limit is invalid")?;
    if limit > MAX_ARCHIVE_PAGE_ITEMS {
        return Err(format!(
            "archive page limit exceeds {MAX_ARCHIVE_PAGE_ITEMS} entries"
        ));
    }
    let offset = usize::try_from(offset).map_err(|_| "archive page offset is invalid")?;
    let mut retained_bytes = 0usize;
    let mut page = Vec::with_capacity(limit.min(entries.len().saturating_sub(offset)));
    for entry in entries.iter().skip(offset).take(limit) {
        retained_bytes = retained_bytes
            .checked_add(entry.path.len())
            .ok_or("archive page text budget overflowed")?;
        if retained_bytes > MAX_ARCHIVE_PAGE_BYTES {
            return Err("archive page exceeds the 1 MiB text budget".to_string());
        }
        page.push(entry.path.clone());
    }
    Ok(page)
}

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
        self.impl_list_archive_files_page(0, MAX_ARCHIVE_PAGE_ITEMS as u32)
    }

    pub(super) fn impl_archive_file_count(&self) -> std::result::Result<u64, String> {
        if let Some(ref ctx) = self.event_context {
            return Ok(archive_entry_count(&ctx.entries));
        }
        let bridge = self
            .active_tab
            .as_ref()
            .ok_or("Active-tab bridge not configured")?;
        if bridge.archive_path().is_none() {
            return Err("No archive currently open".to_string());
        }
        Ok(u64::try_from(bridge.archive_entry_count()).unwrap_or(u64::MAX))
    }

    pub(super) fn impl_list_archive_files_page(
        &self,
        offset: u32,
        limit: u32,
    ) -> std::result::Result<Vec<String>, String> {
        if usize::try_from(limit).unwrap_or(usize::MAX) > MAX_ARCHIVE_PAGE_ITEMS {
            return Err(format!(
                "archive page limit exceeds {MAX_ARCHIVE_PAGE_ITEMS} entries"
            ));
        }

        // Per-event context wins (see `impl_current_archive_info`).
        // The event payload carries the originating tab's entries
        // snapshotted at fire time, so a plugin handler processing
        // a queued event sees that tab's files even when the user
        // has switched tabs in the meantime.
        if let Some(ref ctx) = self.event_context {
            return archive_entry_page(&ctx.entries, offset, limit);
        }

        // Non-event path (panel render, etc.): read the host's
        // authoritative per-tab entries cache via the bridge —
        // populated by `list_archive` at open time so we pay only
        // an `Arc` clone + per-entry `String` clone.
        let bridge = self
            .active_tab
            .as_ref()
            .ok_or("Active-tab bridge not configured")?;
        if bridge.archive_path().is_none() {
            return Err("No archive currently open".to_string());
        }
        Ok(bridge.archive_entries_page(offset as usize, limit as usize))
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

        info!("[HostFunctions] Renamed archive");

        Ok(new_path_str)
    }
}
