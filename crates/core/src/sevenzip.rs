use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind};
use anyhow::{anyhow, Context, Result};
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tracing::{debug, error, info};
use which::which;

#[derive(Clone)]
pub struct SevenZipCli {
    exe: PathBuf,
}

impl SevenZipCli {
    pub fn detect(explicit: Option<&Path>) -> Result<Self> {
        if let Some(p) = explicit {
            info!("Using explicit 7-Zip path: {}", p.display());
            return Ok(Self { exe: p.to_owned() });
        }

        debug!("Searching for 7-Zip executable in PATH");
        for cand in SevenZipCli::candidates() {
            if let Ok(path) = which(cand) {
                info!("Found 7-Zip executable: {}", path.display());
                return Ok(Self { exe: path });
            }
        }

        error!("7-Zip executable not found in PATH");
        Err(anyhow!(
            "7z/7za/7zz not found on PATH. Please install 7-Zip (or provide path in settings)."
        ))
    }

    fn candidates() -> &'static [&'static str] {
        if cfg!(windows) {
            &["7zz.exe", "7z.exe", "7za.exe"]
        } else {
            &["7zz", "7z", "7za"]
        }
    }

    // New: accept any iterator of OsStr-like items
    fn run<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args_vec: Vec<_> = args.into_iter().collect();
        debug!("Executing 7-Zip command: {:?}", self.exe);
        debug!(
            "Command arguments: {:?}",
            args_vec.iter().map(|a| a.as_ref()).collect::<Vec<_>>()
        );

        let out = Command::new(&self.exe)
            .args(&args_vec)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("spawning 7z")?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            error!(
                "7-Zip command failed with code {:?}: {}",
                out.status.code(),
                err.trim()
            );
            error!("7-Zip stderr: {}", err.trim());
            error!("7-Zip stdout: {}", stdout.trim());
            return Err(anyhow!(
                "7z failed (code {:?}): {}",
                out.status.code(),
                err.trim()
            ));
        }

        debug!("7-Zip command completed successfully");
        
        // Log raw bytes for Unicode debugging
        debug!("Output byte length: {}", out.stdout.len());
        if !out.stdout.is_empty() {
            debug!("First 100 bytes (hex): {:x?}", &out.stdout[..out.stdout.len().min(100)]);
        }
        
        // Try to decode as UTF-8
        match String::from_utf8(out.stdout.clone()) {
            Ok(s) => {
                debug!("Successfully decoded output as UTF-8");
                Ok(s)
            }
            Err(e) => {
                error!("UTF-8 decoding failed at byte {}: {:?}", e.utf8_error().valid_up_to(), e);
                debug!("Falling back to lossy UTF-8 conversion (will replace invalid chars with �)");
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
        }
    }

    // Run 7z and feed data to stdin, checking only status.
    fn run_status_with_stdin<I, S>(&self, args: I, stdin_data: &[u8]) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        debug!("Executing 7-Zip command (status+stdin): {:?}", self.exe);
        let mut child = Command::new(&self.exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning 7z with stdin")?;

        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(stdin_data)?;
        }

        let status = child.wait().context("waiting for 7z")?;
        if !status.success() {
            error!("7-Zip command failed with code {:?}", status.code());
            return Err(anyhow!("7z failed (code {:?})", status.code()));
        }
        Ok(())
    }

    fn run_status<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        debug!("Executing 7-Zip command (status mode): {:?}", self.exe);
        let status = Command::new(&self.exe)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .context("spawning 7z")?;

        if !status.success() {
            error!("7-Zip command failed with code {:?}", status.code());
            return Err(anyhow!("7z failed (code {:?})", status.code()));
        }

        debug!("7-Zip command completed successfully");
        Ok(())
    }

    fn parse_kind(slt: &str) -> ArchiveKind {
        for line in slt.lines() {
            if let Some(rest) = line.strip_prefix("Type = ") {
                return match rest.trim().to_lowercase().as_str() {
                    "zip" => ArchiveKind::Zip,
                    "7z" => ArchiveKind::SevenZ,
                    "rar" => ArchiveKind::Rar,
                    other => ArchiveKind::Unknown(other.to_string()),
                };
            }
        }
        ArchiveKind::Unknown("unknown".into())
    }

    fn parse_list_slt(&self, archive_path: &Path, slt: &str) -> ArchiveInfo {
        let mut entries = Vec::new();
        let mut cur: Vec<(String, String)> = Vec::new();
        let mut header_props: HashMap<String, String> = HashMap::new();
        let mut in_entries = false;
        let mut encrypted_methods: BTreeSet<String> = BTreeSet::new();

        let flush = |cur: &Vec<(String, String)>,
                     entries: &mut Vec<ArchiveEntry>,
                     encrypted_methods: &mut BTreeSet<String>| {
            if cur.is_empty() {
                return;
            }

            let mut map = HashMap::new();
            for (k, v) in cur {
                map.insert(k.as_str(), v.as_str());
            }

            let has_path = map.contains_key("Path");
            let has_attributes = map.contains_key("Attributes") || map.contains_key("Folder");
            
            if !has_path || !has_attributes {
                return;
            }

            let mut path = map.get("Path").unwrap_or(&"").to_string();
            if path.starts_with("./") {
                path = path[2..].to_string();
            }
            path = path.replace('\\', "/");
            if path.ends_with('/') && path.len() > 1 {
                path.pop();
                while path.ends_with('/') {
                    path.pop();
                }
            }

            let is_dir = match map.get("Folder") {
                Some(&"+") => true,
                _ => match map.get("Attributes") {
                    Some(attrs) if attrs.contains('D') => true,
                    _ => false,
                },
            };

            let size = map
                .get("Size")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let packed = map
                .get("Packed Size")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let modified = map.get("Modified").map(|s| s.to_string());
            let encrypted = matches!(map.get("Encrypted"), Some(&"+"));

            if encrypted {
                if let Some(method) = map.get("Method") {
                    if !method.trim().is_empty() {
                        encrypted_methods.insert(method.trim().to_string());
                    }
                }
            }

            entries.push(ArchiveEntry {
                path,
                size,
                packed_size: packed,
                modified,
                is_dir,
                encrypted,
            });
        };

        for line in slt.lines() {
            let line = line.trim_end();

            if line.starts_with("----------") {
                in_entries = true;
                continue;
            }

            // Check if this looks like an entry line (has " = " separator)
            if let Some((k, v)) = line.split_once(" = ") {
                let key = k.trim();
                let value = v.trim();
                
                // If we see "Path = ", we're definitely in entries section
                if key == "Path" && !in_entries {
                    in_entries = true;
                }
                
                if in_entries {
                    // This is an entry field
                    cur.push((key.to_string(), value.to_string()));
                } else {
                    // This is a header property
                    header_props.insert(key.to_string(), value.to_string());
                }
                continue;
            }

            // Empty line marks the end of an entry
            if line.is_empty() && in_entries {
                flush(&cur, &mut entries, &mut encrypted_methods);
                cur.clear();
                continue;
            }
        }
        
        // Don't forget to flush the last entry
        flush(&cur, &mut entries, &mut encrypted_methods);

        let mut archive_encrypted = entries.iter().any(|entry| entry.encrypted);

        if let Some(value) = header_props.get("Encrypted") {
            if value == "+" || value.eq_ignore_ascii_case("yes") {
                archive_encrypted = true;
            }
        }

        if let Some(value) = header_props.get("Encryption") {
            if !value.trim().is_empty() {
                archive_encrypted = true;
                encrypted_methods.insert(value.trim().to_string());
            }
        }

        if let Some(value) = header_props.get("Characteristics") {
            if value.to_lowercase().contains("encrypted") {
                archive_encrypted = true;
            }
        }

        let headers_encrypted = matches!(
            header_props.get("Headers Encrypted"),
            Some(value) if value == "+" || value.eq_ignore_ascii_case("yes")
        );

        if headers_encrypted {
            if let Some(value) = header_props.get("Header Encryption") {
                if !value.trim().is_empty() {
                    encrypted_methods.insert(value.trim().to_string());
                }
            }
        }

        let encryption_method = if archive_encrypted || headers_encrypted {
            if !encrypted_methods.is_empty() {
                Some(encrypted_methods.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                header_props.get("Method").cloned()
            }
        } else {
            None
        };

        let kind = Self::parse_kind(slt);
        ArchiveInfo {
            archive_path: archive_path.to_path_buf(),
            archive_kind: kind,
            entries,
            encrypted: archive_encrypted,
            headers_encrypted,
            encryption_method,
        }
    }
}

impl ArchiveBackend for SevenZipCli {
    fn identify(&self, path: &Path) -> Result<ArchiveKind> {
        info!("Identifying archive type: {}", path.display());
        let args = vec![
            OsString::from("l"),
            OsString::from("-ba"),
            OsString::from("-slt"),
            OsString::from("-sccUTF-8"),  // Console charset for output
            OsString::from("-scsUTF-8"),  // Charset for list files
            path.as_os_str().to_os_string(),
        ];
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

        let mut args = vec![
            OsString::from("l"),
            OsString::from("-ba"),
            OsString::from("-slt"),
            OsString::from("-sccUTF-8"),  // Console charset for output
            OsString::from("-scsUTF-8"),  // Charset for list files
        ];
        // Always provide password flag to avoid interactive prompts
        // If no password provided, use empty string which will fail cleanly for encrypted archives
        match password {
            Some(p) => args.push(OsString::from(format!("-p{}", p))),
            None => args.push(OsString::from("-p")), // Empty password to prevent interactive prompt
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
        debug!("Files to extract: {:?}", files);

        let mut args = vec![
            OsString::from("e"), // Note: 'e' extracts without paths, use 'x' to preserve paths
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),  // Console charset
            OsString::from("-scsUTF-8"),  // Charset for list files
        ];

        // Always provide password flag to avoid interactive prompts
        match password {
            Some(p) => {
                debug!("Using password for extraction");
                args.push(OsString::from(format!("-p{}", p)));
            }
            None => args.push(OsString::from("-p")), // Empty password to prevent interactive prompt
        }

        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);

        args.push(path.as_os_str().to_os_string());

        // Add specific files to extract
        for file in files {
            args.push(OsString::from(file));
        }

        self.run_status(args)?;
        info!("Files extracted successfully");
        Ok(())
    }

    fn extract_all(&self, path: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
        info!(
            "Extracting all files from {} to {}",
            path.display(),
            dest.display()
        );

        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),  // Console charset
            OsString::from("-scsUTF-8"),  // Charset for list files
        ];
        // Always provide password flag to avoid interactive prompts
        match password {
            Some(p) => {
                debug!("Using password for extraction");
                args.push(OsString::from(format!("-p{}", p)));
            }
            None => args.push(OsString::from("-p")), // Empty password to prevent interactive prompt
        }
        // Build -o<dest> as a single OsString without leaking
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);

        args.push(path.as_os_str().to_os_string());
        self.run_status(args)?;
        info!("All files extracted successfully");
        Ok(())
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
            OsString::from("-sccUTF-8"),  // Console charset
            OsString::from("-scsUTF-8"),  // Charset for list files
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
            OsString::from("-sccUTF-8"),  // Console charset
            OsString::from("-scsUTF-8"),  // Charset for list files
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
            OsString::from("-sccUTF-8"),  // Console charset
            OsString::from("-scsUTF-8"),  // Charset for list files
        ];

        // Add compression settings for 7z
        if format == "7z" {
            debug!("Using maximum compression for 7z format");
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
        match password {
            Some(p) => args.push(OsString::from(format!("-p{}", p))),
            None => args.push(OsString::from("-p")),
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
}
