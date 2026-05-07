use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;
use unrar::Archive;

/// Native RAR backend using the `unrar` crate (wrapper around official unrar library)
#[derive(Clone)]
pub struct UnrarBackend;

impl UnrarBackend {
    pub fn new() -> Self {
        Self
    }
}

impl ArchiveBackend for UnrarBackend {
    fn name(&self) -> &str {
        "UnRAR (Native)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // RAR backend is read-only
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
        info!("Using {} backend to list: {}", self.name(), path.display());

        // Native listing first — the `unrar` crate hands us proper Unicode
        // filenames via WideCString. We previously delegated the entire
        // listing to UnrarCli to recover packed_size, but UnRAR.exe writes to
        // its piped stdout in the Windows console code page, which mangles
        // CJK characters into '?'. Doing the listing natively keeps the names
        // intact; we then enrich packed_size from a CLI run by entry order.
        let archive = if let Some(pwd) = password {
            info!("Using password for RAR archive listing");
            Archive::with_password(path, pwd.as_bytes())
        } else {
            Archive::new(path)
        };

        let open_archive = archive
            .open_for_listing()
            .context("Failed to open RAR archive for listing")?;

        let mut entries = Vec::new();
        let mut any_encrypted = false;
        let mut headers_encrypted = false;

        for entry_result in open_archive {
            match entry_result {
                Ok(entry) => {
                    let is_dir = entry.is_directory();
                    let encrypted = entry.is_encrypted();

                    if encrypted {
                        any_encrypted = true;
                    }

                    // Check for invalid UTF-8/encoding issues (detect replacement char)
                    let filename = entry.filename.to_string_lossy().into_owned();
                    if filename.contains('\u{FFFD}') {
                        tracing::warn!(
                            "Detected invalid encoding (replacement char) in RAR entry: {}",
                            filename
                        );
                        // Return empty list to trigger fallback to CLI which might handle encoding better (e.g. 7z CLI)
                        let encryption_method = if any_encrypted {
                            Some("RAR".to_string())
                        } else {
                            None
                        };

                        return Ok(ArchiveInfo {
                            archive_path: path.to_path_buf(),
                            archive_kind: ArchiveKind::Rar,
                            entries: Vec::new(),
                            encrypted: any_encrypted,
                            headers_encrypted,
                            encryption_method,
                        });
                    }

                    entries.push(ArchiveEntry {
                        path: filename,
                        size: entry.unpacked_size,
                        packed_size: 0,
                        modified: None, // entry.file_time
                        is_dir,
                        encrypted,
                        // Note: file_crc access might trigger computation if not lazy.
                        // However, standard unrar header usually contains it.
                        // usage of {:08X} is standard.
                        crc32: Some(format!("{:08X}", entry.file_crc)),
                    });
                }
                Err(e) => {
                    // Check if this is a "headers encrypted" error
                    let err_str = e.to_string();
                    if err_str.contains("password") || err_str.contains("encrypted") {
                        headers_encrypted = true;
                        any_encrypted = true;
                        info!("RAR archive has encrypted headers");
                        break;
                    }
                    return Err(anyhow!("Failed to read RAR entry: {}", e));
                }
            }
        }

        let encryption_method = if any_encrypted {
            Some("RAR".to_string())
        } else {
            None
        };

        // Enrich packed_size from the CLI by entry-order zip. The CLI may
        // mangle filenames on non-CJK Windows (OEM stdout codepage) but the
        // pack-size data is correct, and entry order between the unrar crate
        // and UnRAR.exe matches because both call into libunrar internals.
        // Skip silently if the CLI isn't available, fails, or produces a
        // different number of entries.
        if let Some(cli) = crate::backends::unrar_cli::UnrarCli::detect() {
            match cli.list(path, password) {
                Ok(cli_info) if cli_info.entries.len() == entries.len() => {
                    for (entry, cli_entry) in entries.iter_mut().zip(cli_info.entries.iter()) {
                        if cli_entry.packed_size > 0 {
                            entry.packed_size = cli_entry.packed_size;
                        }
                    }
                }
                Ok(cli_info) => {
                    tracing::warn!(
                        "[unrar] Native listing returned {} entries but CLI returned {} — \
                         skipping packed_size enrichment",
                        entries.len(),
                        cli_info.entries.len()
                    );
                }
                Err(e) => {
                    tracing::warn!("[unrar] CLI packed_size enrichment failed: {}", e);
                }
            }
        }

        Ok(ArchiveInfo {
            archive_path: path.to_path_buf(),
            archive_kind: ArchiveKind::Rar,
            entries,
            encrypted: any_encrypted,
            headers_encrypted,
            encryption_method,
        })
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Using {} backend to extract {} to {}",
            self.name(),
            path.display(),
            dest.display()
        );

        std::fs::create_dir_all(dest).context("Failed to create destination directory")?;

        let archive = if let Some(pwd) = password {
            info!("Using password for RAR archive extraction");
            Archive::with_password(path, pwd.as_bytes())
        } else {
            Archive::new(path)
        };

        let mut open_archive = archive
            .open_for_processing()
            .context("Failed to open RAR archive for extraction")?;

        while let Some(header) = open_archive.read_header()? {
            let entry_name = header.entry().filename.to_string_lossy().into_owned();
            let full_dest = dest.join(&entry_name);

            // Check if path is too long for Windows (MAX_PATH = 260)
            // If so, fail early to trigger CLI fallback rather than getting ECreate
            let path_len = full_dest.to_string_lossy().len();
            if path_len > 250 {
                tracing::warn!(
                    "Path too long for native extraction ({} chars): {}",
                    path_len,
                    full_dest.display()
                );
                return Err(anyhow::anyhow!(
                    "Path too long for native UnRAR ({} chars, max 250). Use CLI fallback.",
                    path_len
                ));
            }

            match header.extract_to(dest.to_path_buf()) {
                Ok(next_archive) => {
                    open_archive = next_archive;
                }
                Err(e) => {
                    // Log detailed error info from UnrarError
                    tracing::error!(
                        "UnRAR failed to extract '{}' to '{}': code={:?}, when={:?}",
                        entry_name,
                        dest.display(),
                        e.code,
                        e.when
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to extract '{}': {:?} during {:?}",
                        entry_name,
                        e.code,
                        e.when
                    ));
                }
            }
        }

        Ok(())
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        // Delegate to the progress version with no callback
        self.extract_files_with_progress(path, dest, files, password, None, None)
    }

    fn extract_files_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;

        info!(
            "Using {} backend to extract {} files from RAR with progress",
            self.name(),
            files.len()
        );

        std::fs::create_dir_all(dest)?;

        let archive = if let Some(pwd) = password {
            info!("Using password for RAR file extraction");
            Archive::with_password(path, pwd.as_bytes())
        } else {
            Archive::new(path)
        };

        let mut open_archive = archive.open_for_processing()?;

        let total = files.len();
        let mut processed = 0;

        // Extract only specified files
        while let Some(header) = open_archive.read_header()? {
            // Check for cancellation
            if let Some(token) = cancel {
                if token.load(Ordering::Relaxed) {
                    info!("RAR extraction cancelled by user");
                    return Err(anyhow!("Extraction cancelled by user"));
                }
            }

            let entry_path = header.entry().filename.to_string_lossy().into_owned();

            if files
                .iter()
                .any(|f| entry_path == *f || entry_path.contains(f))
            {
                // Report progress before extraction
                processed += 1;
                if let Some(cb) = progress {
                    let percent = if total > 0 {
                        ((processed * 100) / total) as u8
                    } else {
                        0
                    };
                    cb(crate::ExtractionProgress {
                        current: processed,
                        total,
                        current_file: entry_path.clone(),
                        percent,
                    });
                }

                open_archive = header.extract_to(dest.to_path_buf())?;
            } else {
                open_archive = header.skip()?;
            }
        }

        // Report completion
        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: total,
                total,
                current_file: "Complete".to_string(),
                percent: 100,
            });
        }

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
            "Using {} backend to extract directory '{}' from RAR",
            self.name(),
            dir_path
        );

        std::fs::create_dir_all(dest)?;

        let archive = if let Some(pwd) = password {
            info!("Using password for RAR directory extraction");
            Archive::with_password(path, pwd.as_bytes())
        } else {
            Archive::new(path)
        };

        let mut open_archive = archive.open_for_processing()?;

        // Normalize to forward slashes for comparison - RAR entries may use either separator
        let dir_path_normalized = dir_path
            .trim_end_matches('/')
            .trim_end_matches('\\')
            .replace('\\', "/");
        let dir_prefix = format!("{}/", dir_path_normalized);

        info!(
            "RAR extract_directory: looking for entries starting with '{}' or exactly '{}'",
            dir_prefix, dir_path_normalized
        );

        let mut extracted_count = 0;
        while let Some(header) = open_archive.read_header()? {
            let entry_path = header.entry().filename.to_string_lossy().into_owned();
            // Normalize entry path separators for comparison
            let entry_path_normalized = entry_path.replace('\\', "/");

            if entry_path_normalized.starts_with(&dir_prefix)
                || entry_path_normalized == dir_path_normalized
            {
                extracted_count += 1;
                open_archive = header.extract_to(dest.to_path_buf())?;
            } else {
                open_archive = header.skip()?;
            }
        }

        info!(
            "RAR extract_directory: extracted {} entries",
            extracted_count
        );

        Ok(())
    }

    fn extract(
        &self,
        archive: &Path,
        dest: &Path,
        entries: Option<&[crate::EntryRef<'_>]>,
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;

        // For None or directory entries or large counts, use extract_all
        let should_extract_all = match entries {
            None => true,
            Some(refs) => {
                let has_dir = refs.iter().any(|r| r.is_dir);
                let too_many = refs.len() > 100; // Native can handle more than CLI
                has_dir || too_many
            }
        };

        if should_extract_all {
            info!(
                "[UnRAR Native] Using extract_all for {} entries (dir or large count)",
                entries.map(|e| e.len()).unwrap_or(0)
            );

            if let Some(cb) = progress {
                cb(crate::ExtractionProgress {
                    current: 0,
                    total: 1,
                    current_file: "Extracting all...".to_string(),
                    percent: 0,
                });
            }

            let result = self.extract_all(archive, dest, password);

            if let Some(cb) = progress {
                cb(crate::ExtractionProgress {
                    current: 1,
                    total: 1,
                    current_file: "Complete".to_string(),
                    percent: 100,
                });
            }

            return result;
        }

        // Extract selected entries using proper EntryRef::matches()
        let refs = entries.unwrap();
        info!("[UnRAR Native] Extracting {} specific entries", refs.len());

        std::fs::create_dir_all(dest)?;

        let arc = if let Some(pwd) = password {
            unrar::Archive::with_password(archive, pwd.as_bytes())
        } else {
            unrar::Archive::new(archive)
        };

        let mut open_archive = arc.open_for_processing()?;

        // Count matching entries for progress
        let total = refs.len();
        let mut processed = 0;

        while let Some(header) = open_archive.read_header()? {
            if let Some(token) = cancel {
                if token.load(Ordering::Relaxed) {
                    return Err(anyhow!("Extraction cancelled"));
                }
            }

            let entry_path = header.entry().filename.to_string_lossy().into_owned();

            // Use proper EntryRef matching
            if refs.iter().any(|r| r.matches(&entry_path)) {
                processed += 1;
                if let Some(cb) = progress {
                    let percent = ((processed * 100) / total) as u8;
                    cb(crate::ExtractionProgress {
                        current: processed,
                        total,
                        current_file: entry_path.clone(),
                        percent,
                    });
                }
                open_archive = header.extract_to(dest.to_path_buf())?;
            } else {
                open_archive = header.skip()?;
            }
        }

        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: total,
                total,
                current_file: "Complete".to_string(),
                percent: 100,
            });
        }

        Ok(())
    }

    fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> Result<()> {
        Err(anyhow!(
            "RAR backend is read-only, cannot create 7z archives"
        ))
    }

    fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> Result<()> {
        Err(anyhow!("RAR backend is read-only, cannot modify archives"))
    }

    fn create_archive(&self, _dest: &Path, _files: &[PathBuf], _format: &str) -> Result<()> {
        Err(anyhow!("RAR backend is read-only, cannot create archives"))
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        let temp_dir = tempfile::tempdir()?;
        self.extract_files(
            archive,
            temp_dir.path(),
            &[path_in_archive.to_string()],
            password,
        )?;

        let extracted_file = temp_dir.path().join(path_in_archive);
        let content = std::fs::read_to_string(extracted_file).or_else(|_| {
            // Try lossy UTF-8 if exact UTF-8 fails
            std::fs::read(temp_dir.path().join(path_in_archive))
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        })?;

        Ok(content)
    }

    fn delete_files(&self, _archive: &Path, _files: &[String]) -> Result<()> {
        Err(anyhow!("RAR backend is read-only, cannot delete files"))
    }

    fn add_or_update_file_from_str(
        &self,
        _archive: &Path,
        _path_in_archive: &str,
        _content: &str,
    ) -> Result<()> {
        Err(anyhow!("RAR backend is read-only, cannot modify files"))
    }

    fn convert_to_7z(&self, source: &crate::Archive, _dest: &Path, temp_dir: &Path) -> Result<()> {
        // Extract to temp using Archive handle (with password if needed)
        let extract_dir = temp_dir.join("rar_extract");
        source.extract_all(&extract_dir)?;

        // Caller should use a different backend to compress to 7z
        Err(anyhow!(
            "RAR backend extracted to {:?}, use another backend to create 7z",
            extract_dir
        ))
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        // Get CRC from listing
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
