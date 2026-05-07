//! Selective extraction strategies for the 7-Zip CLI backend.
//!
//! `extract_selective` picks between three strategies based on how many
//! entries the caller wants vs. how many it wants excluded:
//! - all selected → fall back to `extract_all`
//! - few exclusions → exclude-list response file (`-x@file`)
//! - few selections → inline file args (under cmdline length cap)
//! - many selections → include-list response file (`-i@file`)
//!
//! Lifted from `backend.rs` so the trait-impl file stays focused on the
//! `ArchiveBackend` surface; the response-file plumbing is mechanical
//! and stands alone.

use super::backend::{extract_base_args, report_progress, FILE_COUNT_THRESHOLD};
use super::SevenZipCli;
use crate::{ArchiveBackend, ArchiveInfo};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use tracing::info;

impl SevenZipCli {
    /// Extract a specific subset of entries using the optimal strategy.
    pub(super) fn extract_selective(
        &self,
        archive: &Path,
        dest: &Path,
        refs: &[crate::EntryRef<'_>],
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
    ) -> Result<()> {
        let archive_info = self.list(archive, password)?;
        let total_entries = archive_info.entries.len();

        // Build set of selected paths (expanding directories)
        let mut selected_paths: HashSet<String> = HashSet::new();
        for entry_ref in refs {
            if entry_ref.is_dir {
                for archive_entry in &archive_info.entries {
                    if entry_ref.matches(&archive_entry.path) {
                        selected_paths.insert(archive_entry.path.clone());
                    }
                }
            } else {
                selected_paths.insert(entry_ref.path.to_string());
            }
        }

        let selected_count = selected_paths.len();
        let excluded_count = total_entries.saturating_sub(selected_count);

        info!(
            "[7z CLI] Strategy: {} selected, {} excluded, {} total",
            selected_count, excluded_count, total_entries
        );

        if excluded_count == 0 {
            info!("[7z CLI] All entries selected, using extract_all");
            report_progress(progress, 0);
            let result = self.extract_all(archive, dest, password);
            report_progress(progress, 100);
            return result;
        }

        if excluded_count <= FILE_COUNT_THRESHOLD {
            info!("[7z CLI] Using extract_all with {} exclusions", excluded_count);
            return self.extract_with_exclude_file(
                archive, dest, password, progress, &archive_info, &selected_paths,
            );
        }

        if selected_count <= FILE_COUNT_THRESHOLD {
            info!("[7z CLI] Extracting {} files directly on cmd", selected_count);
            let files: Vec<String> = selected_paths.into_iter().collect();
            return self.extract_files_with_progress(
                archive, dest, &files, password, progress, None,
            );
        }

        info!("[7z CLI] Using include response file for {} entries", selected_count);
        self.extract_with_include_file(archive, dest, password, progress, &selected_paths)
    }

    /// Extract using an exclude response file (for few exclusions).
    fn extract_with_exclude_file(
        &self,
        archive: &Path,
        dest: &Path,
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        archive_info: &ArchiveInfo,
        selected_paths: &HashSet<String>,
    ) -> Result<()> {
        let excluded: Vec<String> = archive_info
            .entries
            .iter()
            .filter(|e| !selected_paths.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();

        let mut excludefile =
            tempfile::NamedTempFile::new().context("Creating exclude file for 7z")?;
        for path in &excluded {
            writeln!(excludefile, "{}", path)?;
        }
        excludefile.flush()?;

        let mut args = extract_base_args();
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(archive.as_os_str().to_os_string());
        args.push(OsString::from(format!("-x@{}", excludefile.path().display())));

        report_progress(progress, 0);
        let result = self.run_status(args);
        report_progress(progress, 100);
        result
    }

    /// Extract using an include response file (for many selections).
    fn extract_with_include_file(
        &self,
        archive: &Path,
        dest: &Path,
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        selected_paths: &HashSet<String>,
    ) -> Result<()> {
        let mut includefile =
            tempfile::NamedTempFile::new().context("Creating include file for 7z")?;
        for path in selected_paths {
            writeln!(includefile, "{}", path)?;
        }
        includefile.flush()?;

        let mut args = extract_base_args();
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(archive.as_os_str().to_os_string());
        args.push(OsString::from(format!("-i@{}", includefile.path().display())));

        report_progress(progress, 0);
        let result = self.run_status(args);
        report_progress(progress, 100);
        result
    }
}
