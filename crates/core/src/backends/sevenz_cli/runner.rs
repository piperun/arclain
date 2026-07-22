//! Command execution helpers for 7-Zip CLI

use super::progress::ProgressUpdate;
use super::ChildWithProgress;
use anyhow::{anyhow, Context, Result};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use tracing::{debug, error, info};
use which::which;

/// 7-Zip CLI wrapper for archive operations
#[derive(Clone)]
pub struct SevenZipCli {
    pub(crate) exe: PathBuf,
}

impl SevenZipCli {
    /// Detect 7-Zip executable on the system
    pub fn detect(explicit: Option<&Path>) -> Result<Self> {
        if let Some(p) = explicit {
            info!("Using explicit 7-Zip path: {}", p.display());
            return Ok(Self { exe: p.to_owned() });
        }

        debug!("Searching for 7-Zip executable in PATH");
        for cand in Self::candidates() {
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

    /// Get the path to the 7-Zip executable
    pub fn exe_path(&self) -> &Path {
        &self.exe
    }

    pub(crate) fn candidates() -> &'static [&'static str] {
        if cfg!(windows) {
            &["7zz.exe", "7z.exe", "7za.exe"]
        } else {
            &["7zz", "7z", "7za"]
        }
    }

    /// Spawn 7-Zip with the given args and stream percentage progress via a channel.
    /// Returns the running child process and a receiver for `ProgressUpdate` events.
    pub(crate) fn spawn_with_progress<I, S>(&self, args: I) -> Result<ChildWithProgress>
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
        let mut cmd = Command::new(&self.exe);
        cmd.args(&argv)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let mut child = cmd.spawn().context("spawning 7z (progress)")?;

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

    /// Run 7z command and return stdout as string
    pub(crate) fn run<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args_vec: Vec<_> = args.into_iter().collect();

        let mut cmd = Command::new(&self.exe);
        cmd.args(&args_vec)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let out = cmd.output().context("spawning 7z")?;

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

    /// Run 7z and feed data to stdin, checking only status.
    pub(crate) fn run_status_with_stdin<I, S>(&self, args: I, stdin_data: &[u8]) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        debug!("Executing 7-Zip command (status+stdin): {:?}", self.exe);
        let mut cmd = Command::new(&self.exe);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let mut child = cmd.spawn().context("spawning 7z with stdin")?;

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

    /// Run 7z command and check only exit status
    pub(crate) fn run_status<I, S>(&self, args: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        debug!("Executing 7-Zip command (status mode): {:?}", self.exe);
        let mut cmd = Command::new(&self.exe);
        cmd.args(args).stdout(Stdio::null()).stderr(Stdio::piped());
        crate::utilities::hide_console(&mut cmd);
        let output = cmd.output().context("spawning 7z")?;

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
}
