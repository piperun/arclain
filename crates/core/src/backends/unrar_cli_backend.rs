//! UnRAR CLI backend - shells out to official unrar.exe/UnRAR for RAR extraction
//!
//! This backend is used when the native `unrar` crate fails (e.g., Unicode path issues on Windows).
//! On Windows, it checks for WinRAR's UnRAR.exe in common installation paths.
//! On Linux, it looks for `unrar` in PATH.

use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind, BackendCapabilities};
use anyhow::{anyhow, Context, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tracing::{debug, error, info};
use which::which;

#[derive(Clone)]
pub struct UnrarCli {
    exe: PathBuf,
}

impl UnrarCli {
    /// Try to find UnRAR executable
    pub fn detect() -> Option<Self> {
        // First check PATH
        for candidate in Self::candidates() {
            if let Ok(path) = which(candidate) {
                info!("Found UnRAR executable in PATH: {}", path.display());
                return Some(Self { exe: path });
            }
        }

        // On Windows, also check common WinRAR installation paths
        #[cfg(windows)]
        {
            let winrar_paths = [
                r"C:\Program Files\WinRAR\UnRAR.exe",
                r"C:\Program Files (x86)\WinRAR\UnRAR.exe",
            ];

            for path in winrar_paths {
                let path = PathBuf::from(path);
                if path.exists() {
                    info!("Found UnRAR via WinRAR installation: {}", path.display());
                    return Some(Self { exe: path });
                }
            }
        }

        debug!("UnRAR CLI not found");
        None
    }

    fn candidates() -> &'static [&'static str] {
        if cfg!(windows) {
            &["UnRAR.exe", "unrar.exe"]
        } else {
            &["unrar"]
        }
    }

    fn run(&self, args: &[OsString]) -> Result<String> {
        debug!("Running UnRAR: {:?} {:?}", self.exe, args);

        let output = Command::new(&self.exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("spawning unrar")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("UnRAR failed with code {:?}", output.status.code());
            error!("stderr: {}", stderr.trim());
            error!("stdout: {}", stdout.trim());
            return Err(anyhow!(
                "UnRAR failed (code {:?}): {}",
                output.status.code(),
                stderr.trim()
            ));
        }

        match String::from_utf8(output.stdout) {
            Ok(s) => Ok(s),
            Err(e) => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
        }
    }

    fn run_status(&self, args: &[OsString]) -> Result<()> {
        debug!("Running UnRAR (status mode): {:?} {:?}", self.exe, args);

        let output = Command::new(&self.exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("spawning unrar")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!("UnRAR failed with code {:?}", output.status.code());
            error!("stderr: {}", stderr.trim());
            error!("stdout: {}", stdout.trim());
            return Err(anyhow!(
                "UnRAR failed (code {:?}): {}",
                output.status.code(),
                stderr.trim()
            ));
        }

        Ok(())
    }

    /// Parse unrar listing output (v or vt command)
    fn parse_list_output(&self, archive_path: &Path, output: &str) -> ArchiveInfo {
        let mut entries = Vec::new();
        let mut encrypted = false;
        let mut headers_encrypted = false;

        // UnRAR vt output format has blocks like:
        //   Name: filename
        //   Type: File
        //   Size: 12345
        //   Packed size: 6789
        //   ...

        let mut current_entry: Option<ArchiveEntry> = None;

        for line in output.lines() {
            let line = line.trim();

            if line.starts_with("Name: ") {
                // Flush previous entry
                if let Some(entry) = current_entry.take() {
                    entries.push(entry);
                }

                let path = line.strip_prefix("Name: ").unwrap_or("").to_string();
                current_entry = Some(ArchiveEntry {
                    path,
                    size: 0,
                    packed_size: 0,
                    modified: None,
                    is_dir: false,
                    encrypted: false,
                    crc32: None,
                });
            } else if let Some(ref mut entry) = current_entry {
                if line.starts_with("Type: ") {
                    entry.is_dir = line.contains("Directory");
                } else if line.starts_with("Size: ") {
                    entry.size = line
                        .strip_prefix("Size: ")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                } else if line.starts_with("Packed size: ") {
                    entry.packed_size = line
                        .strip_prefix("Packed size: ")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                } else if line.starts_with("mtime: ") {
                    entry.modified = line.strip_prefix("mtime: ").map(|s| s.to_string());
                } else if line.starts_with("CRC32: ") {
                    entry.crc32 = line.strip_prefix("CRC32: ").map(|s| s.to_uppercase());
                } else if line.starts_with("Flags: ") && line.contains("encrypted") {
                    entry.encrypted = true;
                    encrypted = true;
                }
            }

            // Check for header encryption indicators
            if line.contains("encrypted headers") || line.contains("Encrypted headers") {
                headers_encrypted = true;
                encrypted = true;
            }
        }

        // Flush last entry
        if let Some(entry) = current_entry {
            entries.push(entry);
        }

        ArchiveInfo {
            archive_path: archive_path.to_path_buf(),
            archive_kind: ArchiveKind::Rar,
            entries,
            encrypted,
            headers_encrypted,
            encryption_method: if encrypted {
                Some("RAR".to_string())
            } else {
                None
            },
        }
    }
}

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

        let output = self.run(&args)?;
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
