//! ArchiveBackend trait implementation for UnRAR CLI

use super::UnrarCli;
use crate::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tracing::info;

impl ArchiveBackend for UnrarCli {
    fn name(&self) -> &str {
        "UnRAR (CLI)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // UnRAR is read-only (can't create RAR archives from CLI)
        BackendCapabilities::read_only()
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "rar" | "r00" | "r01" | "r02" | "r03" | "r04" | "r05" => Ok(ArchiveKind::Rar),
            _ => Err(anyhow!("Not a RAR archive: {}", path.display())),
        }
    }

    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo> {
        info!("Using {} to list: {}", self.name(), path.display());

        // Use 'vt' for technical listing (structured output)
        let mut args = vec![
            OsString::from("vt"),  // Technical list
            OsString::from("-c-"), // Disable comments
        ];

        if let Some(pwd) = password {
            args.push(OsString::from(format!("-p{}", pwd)));
        } else {
            // Use "-p-" to indicate "don't ask for password"
            args.push(OsString::from("-p-"));
        }

        args.push(OsString::from(path));

        let output = match self.run(&args) {
            Ok(o) => o,
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("code Some(11)")
                    || err_msg.contains("code 11")
                    || err_msg.contains("Incorrect password")
                {
                    info!("UnRAR CLI indicated encrypted headers (password required)");
                    // Return a dummy info with encrypted headers set
                    return Ok(ArchiveInfo {
                        archive_path: path.to_path_buf(),
                        archive_kind: ArchiveKind::Rar,
                        entries: Vec::new(),
                        encrypted: true,
                        headers_encrypted: true,
                        encryption_method: Some("RAR".to_string()),
                    });
                }
                return Err(e);
            }
        };
        let info = self.parse_list_output(path, &output);

        info!("Listed {} entries from RAR archive", info.entries.len());
        Ok(info)
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Using {} to extract {} to {}",
            self.name(),
            path.display(),
            dest.display()
        );

        std::fs::create_dir_all(dest)?;

        // Use 'x' to extract with full paths
        let mut args = vec![
            OsString::from("x"),    // Extract with full paths
            OsString::from("-o+"),  // Overwrite existing files
            OsString::from("-c-"),  // Disable comments
            OsString::from("-idq"), // Quiet mode (less output)
        ];

        if let Some(pwd) = password {
            info!("Using password for UnRAR CLI extraction");
            args.push(OsString::from(format!("-p{}", pwd)));
        } else {
            args.push(OsString::from("-p-"));
        }

        args.push(OsString::from(path));

        // Destination must end with path separator for unrar
        let mut dest_str = dest.to_string_lossy().into_owned();
        if !dest_str.ends_with(std::path::MAIN_SEPARATOR) {
            dest_str.push(std::path::MAIN_SEPARATOR);
        }
        args.push(OsString::from(dest_str));

        self.run_status(&args)?;
        info!("Extraction completed successfully");
        Ok(())
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} to extract {} files from {}",
            self.name(),
            files.len(),
            path.display()
        );

        std::fs::create_dir_all(dest)?;

        let mut args = vec![
            OsString::from("x"),
            OsString::from("-o+"),
            OsString::from("-c-"),
            OsString::from("-idq"),
        ];

        if let Some(pwd) = password {
            args.push(OsString::from(format!("-p{}", pwd)));
        } else {
            args.push(OsString::from("-p-"));
        }

        args.push(OsString::from(path));

        // Add specific files to extract
        for file in files {
            args.push(OsString::from(file));
        }

        // Destination
        let mut dest_str = dest.to_string_lossy().into_owned();
        if !dest_str.ends_with(std::path::MAIN_SEPARATOR) {
            dest_str.push(std::path::MAIN_SEPARATOR);
        }
        args.push(OsString::from(dest_str));

        self.run_status(&args)?;
        Ok(())
    }

    fn extract_directory(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Using {} to extract directory '{}' from {}",
            self.name(),
            dir_path,
            path.display()
        );

        std::fs::create_dir_all(dest)?;

        let mut args = vec![
            OsString::from("x"),
            OsString::from("-o+"),
            OsString::from("-c-"),
            OsString::from("-idq"),
        ];

        if let Some(pwd) = password {
            args.push(OsString::from(format!("-p{}", pwd)));
        } else {
            args.push(OsString::from("-p-"));
        }

        args.push(OsString::from(path));

        // Add directory pattern with wildcard
        let pattern = format!(
            "{}{}*",
            dir_path.trim_end_matches(['/', '\\']),
            std::path::MAIN_SEPARATOR
        );
        args.push(OsString::from(pattern));

        // Destination
        let mut dest_str = dest.to_string_lossy().into_owned();
        if !dest_str.ends_with(std::path::MAIN_SEPARATOR) {
            dest_str.push(std::path::MAIN_SEPARATOR);
        }
        args.push(OsString::from(dest_str));

        self.run_status(&args)?;
        Ok(())
    }

    fn extract(
        &self,
        archive: &Path,
        dest: &Path,
        entries: Option<&[crate::EntryRef<'_>]>,
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        _cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        use std::collections::HashSet;
        use std::io::Write;

        // Helper for progress
        let report_start = |cb: Option<&crate::ProgressCallback>| {
            if let Some(cb) = cb {
                cb(crate::ExtractionProgress {
                    current: 0,
                    total: 1,
                    current_file: "Extracting...".to_string(),
                    percent: 0,
                });
            }
        };
        let report_done = |cb: Option<&crate::ProgressCallback>| {
            if let Some(cb) = cb {
                cb(crate::ExtractionProgress {
                    current: 1,
                    total: 1,
                    current_file: "Complete".to_string(),
                    percent: 100,
                });
            }
        };

        match entries {
            None => {
                info!("[UnRAR CLI] Extracting all (no entries specified)");
                report_start(progress);
                let result = self.extract_all(archive, dest, password);
                report_done(progress);
                result
            }
            Some(refs) if refs.is_empty() => {
                info!("[UnRAR CLI] No entries to extract");
                Ok(())
            }
            Some(refs) => {
                // Get total entries in archive
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
                    "[UnRAR CLI] Strategy: {} selected, {} excluded, {} total",
                    selected_count, excluded_count, total_entries
                );

                std::fs::create_dir_all(dest)?;

                if excluded_count == 0 {
                    // All entries selected → extract_all
                    info!("[UnRAR CLI] All entries selected, using extract_all");
                    report_start(progress);
                    let result = self.extract_all(archive, dest, password);
                    report_done(progress);
                    result
                } else if excluded_count <= 50 {
                    // Few exclusions → use extract_all with -x@excludefile
                    info!(
                        "[UnRAR CLI] Using extract_all with {} exclusions",
                        excluded_count
                    );

                    let excluded: Vec<String> = archive_info
                        .entries
                        .iter()
                        .filter(|e| !selected_paths.contains(&e.path))
                        .map(|e| e.path.clone())
                        .collect();

                    let mut excludefile = tempfile::NamedTempFile::new()
                        .context("Creating exclude file for unrar")?;
                    for path in &excluded {
                        writeln!(excludefile, "{}", path)?;
                    }
                    excludefile.flush()?;

                    let mut args = vec![
                        OsString::from("x"),
                        OsString::from("-o+"),
                        OsString::from("-c-"),
                        OsString::from("-idq"),
                    ];

                    if let Some(pwd) = password {
                        args.push(OsString::from(format!("-p{}", pwd)));
                    } else {
                        args.push(OsString::from("-p-"));
                    }

                    // Exclude file
                    args.push(OsString::from(format!(
                        "-x@{}",
                        excludefile.path().display()
                    )));

                    args.push(OsString::from(archive));

                    // Destination
                    let mut dest_str = dest.to_string_lossy().into_owned();
                    if !dest_str.ends_with(std::path::MAIN_SEPARATOR) {
                        dest_str.push(std::path::MAIN_SEPARATOR);
                    }
                    args.push(OsString::from(dest_str));

                    report_start(progress);
                    let result = self.run_status(&args);
                    report_done(progress);
                    result
                } else if selected_count <= 50 {
                    // Few selections → list directly on command line
                    info!(
                        "[UnRAR CLI] Extracting {} files directly on cmd",
                        selected_count
                    );
                    let files: Vec<String> = selected_paths.into_iter().collect();
                    self.extract_files(archive, dest, &files, password)
                } else {
                    // Many selections, UnRAR has no include-list support.
                    // Must use extract_all with a LARGE exclude list to be precise.
                    info!(
                        "[UnRAR CLI] Large selection ({} selected, {} excluded). Using extract_all with full exclude list.",
                        selected_count, excluded_count
                    );

                    let excluded: Vec<String> = archive_info
                        .entries
                        .iter()
                        .filter(|e| !selected_paths.contains(&e.path))
                        .map(|e| e.path.clone())
                        .collect();

                    let mut excludefile = tempfile::NamedTempFile::new()
                        .context("Creating exclude file for unrar")?;
                    for path in &excluded {
                        writeln!(excludefile, "{}", path)?;
                    }
                    excludefile.flush()?;

                    let mut args = vec![
                        OsString::from("x"),
                        OsString::from("-o+"),
                        OsString::from("-c-"),
                        OsString::from("-idq"),
                    ];

                    if let Some(pwd) = password {
                        args.push(OsString::from(format!("-p{}", pwd)));
                    } else {
                        args.push(OsString::from("-p-"));
                    }

                    // Exclude file
                    args.push(OsString::from(format!(
                        "-x@{}",
                        excludefile.path().display()
                    )));

                    args.push(OsString::from(archive));

                    // Destination
                    let mut dest_str = dest.to_string_lossy().into_owned();
                    if !dest_str.ends_with(std::path::MAIN_SEPARATOR) {
                        dest_str.push(std::path::MAIN_SEPARATOR);
                    }
                    args.push(OsString::from(dest_str));

                    report_start(progress);
                    let result = self.run_status(&args);
                    report_done(progress);
                    result
                }
            }
        }
    }

    fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> Result<()> {
        Err(anyhow!("UnRAR CLI is read-only, cannot create 7z archives"))
    }

    fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> Result<()> {
        Err(anyhow!("UnRAR CLI is read-only, cannot add files"))
    }

    fn create_archive(&self, _dest: &Path, _files: &[PathBuf], _format: &str) -> Result<()> {
        Err(anyhow!("UnRAR CLI is read-only, cannot create archives"))
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        // Extract to temp file and read
        let temp_dir = tempfile::tempdir()?;
        self.extract_files(
            archive,
            temp_dir.path(),
            &[path_in_archive.to_string()],
            password,
        )?;

        let extracted = temp_dir.path().join(path_in_archive);
        let content = std::fs::read_to_string(&extracted).or_else(|_| {
            std::fs::read(&extracted).map(|b| String::from_utf8_lossy(&b).into_owned())
        })?;

        Ok(content)
    }

    fn delete_files(&self, _archive: &Path, _files: &[String]) -> Result<()> {
        Err(anyhow!("UnRAR CLI is read-only, cannot delete files"))
    }

    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _content: &str,
    ) -> Result<()> {
        Err(anyhow!("UnRAR CLI is read-only, cannot modify archives"))
    }

    fn convert_to_7z(&self, source: &crate::Archive, _dest: &Path, temp_dir: &Path) -> Result<()> {
        // Extract to temp and let caller use another backend
        let extract_dir = temp_dir.join("unrar_extract");
        source.extract_all(&extract_dir)?;
        Err(anyhow!(
            "UnRAR CLI extracted to {:?}, use another backend to create 7z",
            extract_dir
        ))
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        let info = self.list(archive, password)?;

        for entry in info.entries {
            if entry.path == path_in_archive {
                return entry
                    .crc32
                    .ok_or_else(|| anyhow!("No CRC available for entry"));
            }
        }

        Err(anyhow!("Entry not found: {}", path_in_archive))
    }
}
