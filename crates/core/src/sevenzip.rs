use crate::{ArchiveBackend, ArchiveEntry, ArchiveInfo, ArchiveKind};
use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader};
use std::sync::mpsc;
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

    /// Spawn 7-Zip with the given args and stream percentage progress via a channel.
    /// Returns the running child process and a receiver for `ProgressUpdate` events.
    fn spawn_with_progress<I, S>(&self, args: I) -> Result<ChildWithProgress>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        // Ensure we use progress to stderr to keep stdout available for logs
        let mut argv: Vec<OsString> = args
            .into_iter()
            .map(|s| s.as_ref().to_os_string())
            .collect();
        // Add progress flags if not present already
        if !argv.iter().any(|a| a.to_string_lossy().starts_with("-bsp")) {
            argv.push(OsString::from("-bsp2")); // progress to stderr
        }
        // Keep logging minimal to reduce noise
        if !argv.iter().any(|a| a.to_string_lossy().starts_with("-bb")) {
            argv.push(OsString::from("-bb0"));
        }

        debug!("Spawning 7z with progress: {:?} {:?}", self.exe, argv);
        let mut child = Command::new(&self.exe)
            .args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning 7z (progress)")?;

        let stdout = child.stdout.take();
        let (tx, rx) = mpsc::channel::<ProgressUpdate>();

        // Reader thread 1: stdout lines -> log messages
        if let Some(out) = stdout {
            let tx_logs = tx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(out);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line) {
                    if n == 0 {
                        break;
                    }
                    let msg = line.trim_end_matches(['\r', '\n']).to_string();
                    if !msg.is_empty() {
                        let _ = tx_logs.send(ProgressUpdate {
                            percent: 0,
                            message: Some(msg),
                        });
                    }
                    line.clear();
                }
            });
        }

        // Reader thread 2: parse percentages from stderr stream (carriage-return updated)
        if let Some(err) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(err);
                let mut buf: Vec<u8> = Vec::with_capacity(256);
                let mut last_sent: Option<u8> = None;
                loop {
                    match reader.read_until(b'%', &mut buf) {
                        Ok(0) => break,
                        Ok(_) => {
                            if let Some(pos) = buf.iter().rposition(|&b| b == b'%') {
                                let digits_rev: Vec<u8> = buf[..pos]
                                    .iter()
                                    .rev()
                                    .take_while(|b| b.is_ascii_digit())
                                    .copied()
                                    .collect();
                                if !digits_rev.is_empty() {
                                    let mut digits = digits_rev;
                                    digits.reverse();
                                    if let Ok(s) = std::str::from_utf8(&digits) {
                                        if let Ok(mut p) = s.parse::<u8>() {
                                            if p > 100 {
                                                p = 100;
                                            }
                                            if last_sent != Some(p) {
                                                let _ = tx.send(ProgressUpdate {
                                                    percent: p,
                                                    message: None,
                                                });
                                                last_sent = Some(p);
                                            }
                                        }
                                    }
                                }
                            }
                            buf.clear();
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(ProgressUpdate {
                    percent: 100,
                    message: None,
                });
            });
        }

        Ok(ChildWithProgress { child, rx })
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

        match String::from_utf8(out.stdout) {
            Ok(s) => Ok(s),
            Err(e) => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
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
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning 7z with stdin")?;

        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(stdin_data)?;
        }

        let output = child.wait_with_output().context("waiting for 7z")?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            error!(
                "7-Zip command failed with code {:?}: {}",
                output.status.code(),
                err
            );
            return Err(anyhow!(
                "7z failed (code {:?}): {}",
                output.status.code(),
                err
            ));
        }
        Ok(())
    }

    fn run_status<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        debug!("Executing 7-Zip command (status mode): {:?}", self.exe);
        let output = Command::new(&self.exe)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("spawning 7z")?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            error!(
                "7-Zip command failed with code {:?}: {}",
                output.status.code(),
                err
            );
            return Err(anyhow!(
                "7z failed (code {:?}): {}",
                output.status.code(),
                err
            ));
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
            let crc32 = map
                .get("CRC")
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty());
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
                crc32,
            });
        };

        for line in slt.lines() {
            let line = line.trim_end();

            if line.starts_with("----------") {
                in_entries = true;
                continue;
            }

            // Key/value line
            if let Some((k, v)) = line.split_once(" = ") {
                let key = k.trim();
                let value = v.trim();

                // Starting a new entry block on Path
                if key == "Path" {
                    if !cur.is_empty() {
                        flush(&cur, &mut entries, &mut encrypted_methods);
                        cur.clear();
                    }
                    in_entries = true;
                    cur.push((key.to_string(), value.to_string()));
                    continue;
                }

                // Header-level keys that may appear after entries too
                let is_header_key = matches!(
                    key,
                    "Headers Encrypted"
                        | "Encryption"
                        | "Encrypted"
                        | "Header Encryption"
                        | "Characteristics"
                );

                if in_entries && !cur.is_empty() && !is_header_key {
                    // Entry field while inside an entry block
                    cur.push((key.to_string(), value.to_string()));
                } else {
                    // Treat as header property (also captures footer header lines)
                    header_props.insert(key.to_string(), value.to_string());
                }
                continue;
            }

            // Empty line: end current entry block if any
            if line.is_empty() {
                if !cur.is_empty() {
                    flush(&cur, &mut entries, &mut encrypted_methods);
                    cur.clear();
                }
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

        // Detect header encryption across variants
        let mut headers_encrypted = matches!(
            header_props.get("Headers Encrypted"),
            Some(value) if value == "+" || value.eq_ignore_ascii_case("yes")
        );

        // Some formats expose explicit header encryption method without a boolean flag
        if let Some(value) = header_props.get("Header Encryption") {
            if !value.trim().is_empty() {
                headers_encrypted = true;
                encrypted_methods.insert(value.trim().to_string());
            }
        }

        // Fallback: some variants put hints in Characteristics
        if let Some(value) = header_props.get("Characteristics") {
            let lc = value.to_lowercase();
            if lc.contains("headers encrypted") || lc.contains("encrypted headers") {
                headers_encrypted = true;
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
            OsString::from("-sccUTF-8"), // Console charset for output
            OsString::from("-scsUTF-8"), // Charset for list files
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
            OsString::from("-sccUTF-8"), // Console charset for output
            OsString::from("-scsUTF-8"), // Charset for list files
        ];
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
        debug!("Files to extract: {:?}", files);

        let mut args = vec![
            OsString::from("x"), // Use 'x' to preserve directory structure (matches UI path expectations)
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"), // Console charset
            OsString::from("-scsUTF-8"), // Charset for list files
        ];

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
            OsString::from("-sccUTF-8"), // Console charset
            OsString::from("-scsUTF-8"), // Charset for list files
        ];
        // Provide password flag; use empty to avoid interactive prompt when unknown
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
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

        let mut args = vec![
            OsString::from("x"), // Use 'x' to preserve directory structure
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-bd"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];

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

    fn convert_to_7z(&self, source: &Path, dest: &Path, temp_dir: &Path) -> Result<()> {
        info!(
            "Converting {} to 7z at {} (temp: {})",
            source.display(),
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

        // 1. Extract source to work_dir
        self.extract_all(source, &work_dir, None)
            .context("extracting source archive")?;

        // 2. Compress work_dir contents to dest
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
        let status = Command::new(&self.exe)
            .args(&args)
            .current_dir(&work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("spawning 7z for conversion")?;

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

        let mut child = Command::new(&self.exe)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning 7z for crc")?;

        let mut hasher = crc32fast::Hasher::new();
        if let Some(mut stdout) = child.stdout.take() {
            use std::io::Read;
            let mut buf = [0u8; 8192];
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
}

impl SevenZipCli {
    /// Like `extract_files`, but returns a running process with progress updates.
    pub fn spawn_extract_files_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        for f in files {
            args.push(OsString::from(f));
        }
        self.spawn_with_progress(args)
    }

    /// Like `extract_all`, but returns a running process with progress updates.
    pub fn spawn_extract_all_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        self.spawn_with_progress(args)
    }

    /// Like `extract_directory`, but returns a running process with progress updates.
    pub fn spawn_extract_directory_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        if !dir_path.is_empty() {
            let pattern = format!("{}/*", dir_path.trim_end_matches('/'));
            args.push(OsString::from(pattern));
        }
        self.spawn_with_progress(args)
    }
}

/// Progress event from 7-Zip streaming output.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub percent: u8,             // 0..=100
    pub message: Option<String>, // reserved for future use
}

/// Handle for a running 7-Zip process with progress updates.
pub struct ChildWithProgress {
    pub child: std::process::Child,
    pub rx: mpsc::Receiver<ProgressUpdate>,
}
