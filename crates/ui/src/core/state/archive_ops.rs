//! Archive operations - reading and writing files inside an already-open
//! archive.
//!
//! Opening an archive itself (`list_archive`/`list_with_password` in
//! earlier revisions of this file) moved onto
//! `arclain_app::ArclainApp::start_open_archive`, driven by
//! `crate::core::operation_bridge` and started via
//! `crate::core::operations::archive::start_archive_open` -- see that
//! module for the full open flow. What remains here are the archive
//! mutations/reads that operate on an archive already open in a tab.

use super::AppState;
use anyhow::Result;
use std::path::{Path, PathBuf};

impl AppState {
    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        let tab = self.signals.tabs.get().active().clone();
        tab.navigation.get().filter_entries(&tab.entries.get())
    }

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let archive_name = archive.to_str();
        let auto_pw = arclain_core::utilities::auto_password_for(
            &self.pass_rules,
            archive_name,
            &self.last_entries,
        );
        let signal_pw = self.signals.tabs.get().active().current_password.get();
        let pw = signal_pw.as_deref().or(auto_pw.as_deref());
        let backend = self.backend_selector.select(archive)?;
        backend.read_text_file(archive, path_in_archive, pw)
    }

    pub fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.delete_files(archive, files)
    }

    pub fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_or_update_file_from_str(archive, path_in_archive, content)
    }
}
