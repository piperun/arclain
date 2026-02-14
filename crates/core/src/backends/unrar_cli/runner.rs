//! UnRAR CLI command execution helpers

use anyhow::{anyhow, Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing::{debug, error, info};
use which::which;

/// Windows CreateProcess practical command-line length limit (~8 KB).
const CMD_LINE_LENGTH_LIMIT: usize = 8000;

/// UnRAR CLI wrapper for RAR archive extraction
#[derive(Clone)]
pub struct UnrarCli {
    pub(crate) exe: PathBuf,
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

        // On Windows, also check common installation paths
        #[cfg(windows)]
        {
            let mut paths_to_check = vec![
                // WinRAR installations
                PathBuf::from(r"C:\Program Files\WinRAR\UnRAR.exe"),
                PathBuf::from(r"C:\Program Files (x86)\WinRAR\UnRAR.exe"),
            ];

            // Check scoop installation
            if let Some(home) = std::env::var_os("USERPROFILE") {
                let scoop_path = PathBuf::from(home).join(r"scoop\apps\unrar\current\UnRAR.exe");
                paths_to_check.push(scoop_path);

                // Also check scoop shims
                let scoop_shim = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
                    .join(r"scoop\shims\unrar.exe");
                paths_to_check.push(scoop_shim);
            }

            // Check chocolatey installation
            if let Some(choco) = std::env::var_os("ChocolateyInstall") {
                let choco_path = PathBuf::from(choco).join(r"bin\unrar.exe");
                paths_to_check.push(choco_path);
            } else {
                // Default chocolatey location
                paths_to_check.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin\unrar.exe"));
            }

            // Check portable/common locations
            paths_to_check.push(PathBuf::from(r"C:\Tools\unrar.exe"));
            paths_to_check.push(PathBuf::from(r"C:\unrar\unrar.exe"));

            for path in paths_to_check {
                if path.exists() {
                    info!("Found UnRAR at: {}", path.display());
                    return Some(Self { exe: path });
                } else {
                    debug!("UnRAR not found at: {}", path.display());
                }
            }
        }

        debug!("UnRAR CLI not found in any known location");
        None
    }

    pub(crate) fn candidates() -> &'static [&'static str] {
        if cfg!(windows) {
            &["UnRAR.exe", "unrar.exe"]
        } else {
            &["unrar"]
        }
    }

    pub(crate) fn run(&self, args: &[OsString]) -> Result<String> {
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

    pub(crate) fn run_status(&self, args: &[OsString]) -> Result<()> {
        // Calculate approximate command line length
        let cmd_len: usize =
            self.exe.as_os_str().len() + args.iter().map(|a| a.len() + 1).sum::<usize>();
        debug!(
            "Running UnRAR (status mode): {:?} with {} args, ~{} bytes",
            self.exe,
            args.len(),
            cmd_len
        );

        if cmd_len > CMD_LINE_LENGTH_LIMIT {
            error!(
                "Command line too long for UnRAR: {} bytes (limit ~8000). {} args passed.",
                cmd_len,
                args.len()
            );
            return Err(anyhow!(
                "Command line too long ({} bytes). Too many files ({} args) for UnRAR CLI - use extract_all instead",
                cmd_len,
                args.len()
            ));
        }

        let output = Command::new(&self.exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| {
                format!("spawning unrar at {:?} with {} args", self.exe, args.len())
            })?;

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
}
