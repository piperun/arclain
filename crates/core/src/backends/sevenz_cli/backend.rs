//! ArchiveBackend trait implementation for 7-Zip CLI

use super::SevenZipCli;
use crate::{ArchiveBackend, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, error, info};

/// Windows CreateProcess practical command-line length limit (~8 KB).
const CMD_LINE_LENGTH_LIMIT: usize = 8000;

/// Threshold for choosing between inline args vs response file strategies.
pub(super) const FILE_COUNT_THRESHOLD: usize = 50;

/// Buffer size for CRC-32 streaming computation.
const CRC_STREAM_BUFFER: usize = 8192;

/// Buffer size for entry-to-writer streaming extraction.
const EXTRACT_STREAM_BUFFER: usize = 65536;

/// Base args for list/identify commands: `l -ba -slt -sccUTF-8 -scsUTF-8`
fn list_base_args() -> Vec<OsString> {
    vec![
        OsString::from("l"),
        OsString::from("-ba"),
        OsString::from("-slt"),
        OsString::from("-sccUTF-8"),
        OsString::from("-scsUTF-8"),
    ]
}

/// Base args for extract commands: `x -y -mmt=on -sccUTF-8 -scsUTF-8`
pub(super) fn extract_base_args() -> Vec<OsString> {
    vec![
        OsString::from("x"),
        OsString::from("-y"),
        OsString::from("-mmt=on"),
        OsString::from("-sccUTF-8"),
        OsString::from("-scsUTF-8"),
    ]
}

impl ArchiveBackend for SevenZipCli {
    fn name(&self) -> &str {
        "7z (CLI)"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // CLI backend supports full read/write operations
        BackendCapabilities::full_featured()
    }

    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        info!("Identifying archive type: {}", path.display());
        let mut args = list_base_args();
        args.push(path.as_os_str().to_os_string());
        let out = self.run(args)?;
        let kind = Self::parse_kind(&out);
        debug!("Archive type identified: {:?}", kind);
        Ok(kind)
    }

    fn list(&self, path: &Path, password: Option<&str>) -> Result<ArchiveInfo> {
        info!("Listing archive contents: {}", path.display());
        if password.is_some() {
            debug!("Using password for archive listing");
        }

        let mut args = list_base_args();
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            // Suppress interactive password prompt; make 7-Zip fail fast (code 2) on encrypted headers
            args.push(OsString::from("-p"));
        }
        args.push(path.as_os_str().to_os_string());
        let out = self.run(args)?;
        let info = self.parse_list_slt(path, &out);
        info!(
            "Archive listing completed: {} entries found",
            info.entries.len()
        );
        Ok(info)
    }

    fn extract_files(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<()> {
        info!(
            "Extracting {} files from {} to {}",
            files.len(),
            path.display(),
            dest.display()
        );
        debug!(
            "Files to extract (first 10): {:?}",
            files.iter().take(10).collect::<Vec<_>>()
        );

        let mut args = extract_base_args();
        args.push(OsString::from("-bd"));

        // Provide password flag; use empty to avoid interactive prompt when unknown
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }

        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);

        args.push(path.as_os_str().to_os_string());

        // Add specific files to extract, respecting command line length limits
        let mut total_arg_len: usize = args.iter().map(|a| a.len() + 1).sum();

        for file in files {
            let file_os = OsString::from(file);
            let new_len = total_arg_len + file_os.len() + 1;

            if new_len > CMD_LINE_LENGTH_LIMIT && !args.iter().any(|a| a == &file_os) {
                error!("Command line too long! Truncating file list. This may cause incomplete extraction.");
                break;
            }

            args.push(file_os);
            total_arg_len = new_len;
        }

        self.run_status(args)?;

        // Verify extraction worked by checking if at least some files exist
        let sample_files: Vec<_> = files.iter().take(3).collect();
        let mut found_count = 0;
        for file in &sample_files {
            let full_path = dest.join(file);
            if full_path.exists() {
                found_count += 1;
            } else {
                debug!("Sample file not found after extraction: {:?}", full_path);
            }
        }

        if found_count == 0 && !sample_files.is_empty() {
            error!(
                "CRITICAL: 7z extraction returned success but 0/{} sample files found in {:?}",
                sample_files.len(),
                dest
            );
            // List what IS in dest to help debug
            if let Ok(entries) = std::fs::read_dir(dest) {
                for entry in entries.take(5) {
                    if let Ok(e) = entry {
                        error!("  Found in dest: {:?}", e.path());
                    }
                }
            }
        } else {
            info!(
                "Files extracted successfully ({}/{} sample files verified)",
                found_count,
                sample_files.len()
            );
        }

        Ok(())
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Extracting all files from {} to {}",
            path.display(),
            dest.display()
        );

        let mut args = extract_base_args();
        args.push(OsString::from("-bd"));
        // Provide password flag; use empty to avoid interactive prompt when unknown
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);

        args.push(path.as_os_str().to_os_string());
        self.run_status(args)?;
        info!("All files extracted successfully");
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
            "Extracting directory {} from {} to {}",
            dir_path,
            path.display(),
            dest.display()
        );

        let mut args = extract_base_args();
        args.push(OsString::from("-bd"));

        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }

        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);

        args.push(path.as_os_str().to_os_string());

        // Add wildcard pattern to extract directory and its contents
        // If dir_path is empty, extract everything; otherwise extract dir/*
        if dir_path.is_empty() {
            // Extract everything
            debug!("Extracting all files (empty directory path)");
        } else {
            // Extract specific directory with wildcard
            let pattern = format!("{}/*", dir_path.trim_end_matches('/'));
            debug!("Using extraction pattern: {}", pattern);
            args.push(OsString::from(pattern));
        }

        self.run_status(args)?;
        info!("Directory extracted successfully");
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
        match entries {
            None => {
                info!("[7z CLI] Extracting all (no entries specified)");
                report_progress(progress, 0);
                let result = self.extract_all(archive, dest, password);
                report_progress(progress, 100);
                result
            }
            Some(refs) if refs.is_empty() => {
                info!("[7z CLI] No entries to extract");
                Ok(())
            }
            Some(refs) => {
                self.extract_selective(archive, dest, refs, password, progress)
            }
        }
    }

    fn recompress_7z(&self, source: &Path, dest_7z: &Path) -> Result<()> {
        info!(
            "Recompressing {} to 7z format: {}",
            source.display(),
            dest_7z.display()
        );
        debug!("Using maximum compression settings (LZMA2, mx=9)");

        let args = vec![
            OsString::from("a"),
            OsString::from("-t7z"),
            OsString::from("-m0=LZMA2"),
            OsString::from("-mx=9"),
            OsString::from("-mfb=273"),
            OsString::from("-md=256m"),
            OsString::from("-ms=on"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"), // Console charset
            OsString::from("-scsUTF-8"), // Charset for list files
            dest_7z.as_os_str().to_os_string(),
            source.as_os_str().to_os_string(),
        ];
        self.run_status(args)?;
        info!("Recompression completed successfully");
        Ok(())
    }

    fn add_files(&self, archive: &Path, files: &[PathBuf]) -> Result<()> {
        info!(
            "Adding {} files to archive: {}",
            files.len(),
            archive.display()
        );
        debug!("Files to add: {:?}", files);

        let mut args = vec![
            OsString::from("a"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"), // Console charset
            OsString::from("-scsUTF-8"), // Charset for list files
            archive.as_os_str().to_os_string(),
        ];

        for file in files {
            args.push(file.as_os_str().to_os_string());
        }

        self.run_status(args)?;
        info!("Files added to archive successfully");
        Ok(())
    }

    fn create_archive(&self, dest: &Path, files: &[PathBuf], format: &str) -> Result<()> {
        info!(
            "Creating {} archive: {} with {} files",
            format,
            dest.display(),
            files.len()
        );
        debug!("Files to archive: {:?}", files);

        let mut args = vec![
            OsString::from("a"),
            OsString::from(format!("-t{}", format)), // -tzip, -t7z, etc.
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"), // Console charset
            OsString::from("-scsUTF-8"), // Charset for list files
        ];

        // Add compression settings for 7z
        if format == "7z" {
            info!("Using maximum compression (level 9, LZMA2) - this may take longer but creates smaller archives");
            args.push(OsString::from("-mx=9"));
            args.push(OsString::from("-m0=LZMA2"));
        }

        args.push(dest.as_os_str().to_os_string());

        for file in files {
            args.push(file.as_os_str().to_os_string());
        }

        self.run_status(args)?;
        info!("Archive created successfully");
        Ok(())
    }

    fn create_archive_with_profile(
        &self,
        dest: &Path,
        files: &[PathBuf],
        profile: &crate::features::organization::ArchiveProfile,
    ) -> Result<()> {
        use crate::features::organization::ArchiveFormat;

        info!(
            "Creating {} archive with profile '{}': {} with {} files",
            profile.format.display_name(),
            profile.name,
            dest.display(),
            files.len()
        );
        debug!("Profile settings: level={}, method={:?}, solid={}",
            profile.compression_level,
            profile.compression_method,
            profile.solid_archive
        );

        let mut args = vec![
            OsString::from("a"),
            OsString::from(format!("-t{}", profile.format.format_arg())),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];

        // Compression level (0-9)
        args.push(OsString::from(format!("-mx={}", profile.compression_level)));

        // Format-specific options
        match profile.format {
            ArchiveFormat::SevenZ => {
                // Compression method
                if let Some(ref method) = profile.compression_method {
                    args.push(OsString::from(format!("-m0={}", method)));
                }
                // Solid archive
                if profile.solid_archive {
                    args.push(OsString::from("-ms=on"));
                } else {
                    args.push(OsString::from("-ms=off"));
                }
                // Header encryption (requires password, but set the flag)
                if profile.encrypt_headers {
                    args.push(OsString::from("-mhe=on"));
                }
            }
            ArchiveFormat::Zip => {
                // Zip compression method
                if let Some(ref method) = profile.compression_method {
                    // Map method names to 7z zip method identifiers
                    let method_arg = match method.to_lowercase().as_str() {
                        "deflate" => "Deflate",
                        "deflate64" => "Deflate64",
                        "bzip2" => "BZip2",
                        "lzma" => "LZMA",
                        _ => "Deflate",
                    };
                    args.push(OsString::from(format!("-mm={}", method_arg)));
                }
            }
        }

        args.push(dest.as_os_str().to_os_string());

        for file in files {
            args.push(file.as_os_str().to_os_string());
        }

        self.run_status(args)?;
        info!("Archive created successfully with profile '{}'", profile.name);
        Ok(())
    }

    fn convert_to_7z(&self, source: &crate::Archive, dest: &Path, temp_dir: &Path) -> Result<()> {
        info!(
            "Converting {} to 7z at {} (temp: {})",
            source.path().display(),
            dest.display(),
            temp_dir.display()
        );

        // Create a unique temporary directory
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let work_dir = temp_dir.join(format!("arclain_convert_{}", timestamp));
        std::fs::create_dir_all(&work_dir).context("creating temp dir for conversion")?;

        // RAII guard for cleanup
        struct TempDirGuard {
            path: PathBuf,
        }
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                if let Err(e) = std::fs::remove_dir_all(&self.path) {
                    error!("Failed to cleanup temp dir {}: {}", self.path.display(), e);
                }
            }
        }
        let _guard = TempDirGuard {
            path: work_dir.clone(),
        };

        // 1. Extract source archive using its password (if any)
        source
            .extract_all(&work_dir)
            .context("extracting source archive")?;

        // 2. Compress work_dir contents to dest using 7z CLI
        // We run 7z from within work_dir to ensure relative paths are correct
        let dest_abs = std::fs::canonicalize(dest.parent().unwrap_or(Path::new(".")))?
            .join(dest.file_name().unwrap());

        let args = vec![
            OsString::from("a"),
            OsString::from("-t7z"),
            OsString::from("-mx=9"),
            OsString::from("-m0=LZMA2"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
            dest_abs.as_os_str().to_os_string(),
            OsString::from("."), // Add everything in CWD
        ];

        debug!("Executing 7-Zip conversion command: {:?}", args);
        let mut cmd = Command::new(&self.exe);
        cmd.args(&args)
            .current_dir(&work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::utilities::hide_console(&mut cmd);
        let status = cmd.status().context("spawning 7z for conversion")?;

        if !status.success() {
            error!("7-Zip conversion failed with code {:?}", status.code());
            return Err(anyhow!("7z conversion failed (code {:?})", status.code()));
        }

        info!("Conversion completed successfully");
        Ok(())
    }

    fn crc32_of_entry(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        info!(
            "Computing CRC-32 via streaming: {} -> {}",
            archive.display(),
            path_in_archive
        );

        let mut args = vec![
            OsString::from("e"),
            OsString::from("-so"),
            OsString::from("-y"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            // Avoid interactive prompt; fail fast if password is required
            args.push(OsString::from("-p"));
        }
        args.push(archive.as_os_str().to_os_string());
        args.push(OsString::from(path_in_archive));

        let mut cmd = Command::new(&self.exe);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let mut child = cmd.spawn().context("spawning 7z for crc")?;

        let mut hasher = crc32fast::Hasher::new();
        if let Some(mut stdout) = child.stdout.take() {
            use std::io::Read;
            let mut buf = [0u8; CRC_STREAM_BUFFER];
            loop {
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }

        let output = child.wait_with_output().context("waiting for 7z output")?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            error!(
                "7-Zip stream CRC failed with code {:?}: {}",
                output.status.code(),
                err.trim()
            );
            return Err(anyhow!(
                "7z failed (code {:?}): {}",
                output.status.code(),
                err.trim()
            ));
        }

        let sum = hasher.finalize();
        Ok(format!("{:08X}", sum))
    }

    fn read_text_file(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
    ) -> Result<String> {
        info!(
            "Reading file from archive to text: {} -> {}",
            archive.display(),
            path_in_archive
        );
        let mut args = vec![
            OsString::from("e"),
            OsString::from("-so"),
            OsString::from("-y"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        args.push(archive.as_os_str().to_os_string());
        args.push(OsString::from(path_in_archive));
        // Use run() which returns String with UTF-8 (lossy fallback)
        self.run(args)
    }

    fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        info!("Deleting {} files from {}", files.len(), archive.display());
        let mut args = vec![
            OsString::from("d"),
            OsString::from("-y"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
            archive.as_os_str().to_os_string(),
        ];
        for f in files {
            args.push(OsString::from(f));
        }
        self.run_status(args)
    }

    fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        info!(
            "Adding/updating file in archive via stdin: {} -> {}",
            archive.display(),
            path_in_archive
        );
        let args = vec![
            OsString::from("a"),
            OsString::from("-y"),
            OsString::from("-bd"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
            archive.as_os_str().to_os_string(),
            OsString::from(format!("-si{}", path_in_archive)),
        ];
        self.run_status_with_stdin(args, content.as_bytes())
    }

    fn extract_entry_to_writer(
        &self,
        archive: &Path,
        path_in_archive: &str,
        password: Option<&str>,
        writer: &mut dyn std::io::Write,
    ) -> Result<()> {
        debug!(
            "Streaming entry via CLI: {} -> {}",
            archive.display(),
            path_in_archive
        );

        let mut args = vec![
            OsString::from("e"),
            OsString::from("-so"), // Stream to stdout
            OsString::from("-y"),
            OsString::from("-bd"),
            OsString::from("-spf2"), // Disable wildcard matching - treat path as literal
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        args.push(archive.as_os_str().to_os_string());
        // Use -- to mark end of switches and treat the path literally
        args.push(OsString::from("--"));
        args.push(OsString::from(path_in_archive));

        let mut cmd = Command::new(&self.exe);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let mut child = cmd.spawn().context("spawning 7z for streaming")?;

        let mut bytes_written = 0usize;
        if let Some(mut stdout) = child.stdout.take() {
            use std::io::Read;
            let mut buf = [0u8; EXTRACT_STREAM_BUFFER];
            loop {
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buf[..n])?;
                bytes_written += n;
            }
        }

        // Use wait() instead of wait_with_output() since we already consumed stdout
        let status = child.wait().context("waiting for 7z")?;
        if !status.success() {
            // Try to read stderr if still available
            error!("7-Zip streaming failed with code {:?}", status.code());
            return Err(anyhow!("7z streaming failed (code {:?})", status.code()));
        }

        debug!("CLI streaming complete: {} bytes written", bytes_written);

        Ok(())
    }
}

/// Report extraction progress (0 = starting, 100 = complete).
pub(super) fn report_progress(cb: Option<&crate::ProgressCallback>, percent: u8) {
    if let Some(cb) = cb {
        let (current, msg) = if percent >= 100 {
            (1, "Complete")
        } else {
            (0, "Extracting...")
        };
        cb(crate::ExtractionProgress {
            current,
            total: 1,
            current_file: msg.to_string(),
            percent,
        });
    }
}
