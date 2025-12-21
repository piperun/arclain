//! Archive operations

use super::HostFunctions;
use crate::types::PluginCapability;
use std::path::Path;

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

    #[allow(dead_code)]
    pub(super) fn impl_file_read(
        &mut self,
        archive: String,
        file: String,
    ) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::FileRead) {
            return Err("FileRead capability not granted".to_string());
        }
        let backend = self
            .archive_backend
            .as_ref()
            .ok_or("Archive backend not available")?;
        let password = self.current_password.lock().clone();

        backend
            .read_text_file(Path::new(&archive), &file, password.as_deref())
            .map_err(|e| e.to_string())
    }

    #[allow(dead_code)]
    pub(super) fn impl_file_write(
        &mut self,
        archive: String,
        file: String,
        data: String,
    ) -> std::result::Result<String, String> {
        if !self.check_capability(PluginCapability::FileWrite) {
            return Err("FileWrite capability not granted".to_string());
        }
        let backend = self
            .archive_backend
            .as_ref()
            .ok_or("Archive backend not available")?;

        backend
            .add_or_update_file_from_str(Path::new(&archive), &file, &data)
            .map_err(|e| e.to_string())?;
        Ok("Success".to_string())
    }
}
