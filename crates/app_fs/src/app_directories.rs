//! Cross-platform app directory layout — config/cache/data/secrets/
//! plugins/logs/temp paths, plus the side-effect of creating them at
//! startup with owner-only (0o700 on Unix) permissions.
//!
//! Wraps the [`dirs`] crate for path discovery (XDG on Linux,
//! `%APPDATA%` / `%LOCALAPPDATA%` on Windows, `~/Library/...` on
//! macOS) and pairs it with [`ensure_owner_dir`] from this crate so
//! every directory arclain creates for user data is owner-only by
//! default — not just the secrets dir.
//!
//! This is a behavior tightening from the pre-migration version,
//! which only chmod'd `secrets_dir` and left `cache_dir`,
//! `plugins_dir`, `logs_dir`, etc. on whatever the umask gave us
//! (typically `0o755`). The other dirs hold the same kind of
//! per-user state (cached metadata, plugin binaries, log paths
//! that may include filenames) and deserve the same treatment.
//!
//! [`ensure_owner_dir`]: super::ensure_owner_dir

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::ensure_owner_dir;

/// All on-disk directories arclain creates at startup. The struct
/// itself is just paths; construction via [`AppDirectories::init`]
/// is where the side-effect of `mkdir` + `chmod` happens.
#[derive(Debug, Clone)]
pub struct AppDirectories {
    pub config_dir: PathBuf,
    pub databases_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub temp_dir: PathBuf,
}

/// Optional overrides for the three base directories that
/// [`AppDirectories::init`] otherwise derives from the OS. Useful for
/// portable-mode installs (point everything at a single relocatable
/// folder) and for tests that need to redirect into a temp dir.
#[derive(Debug, Clone, Default)]
pub struct PathOverrides {
    pub config_home: Option<PathBuf>,
    pub cache_home: Option<PathBuf>,
    pub data_home: Option<PathBuf>,
}

impl AppDirectories {
    /// Resolve OS-conventional paths under `app_name`, create them
    /// (with `0o700` on Unix), and return the populated struct.
    ///
    /// Call once at startup. Idempotent — re-running on an existing
    /// install is a no-op apart from re-asserting the permission
    /// bits.
    pub fn init(app_name: &str, overrides: Option<PathOverrides>) -> Result<Self> {
        let overrides = overrides.unwrap_or_default();

        // 1. Calculate base paths. Fall back to `.` if the OS can't
        //    provide a home — better to put data in the cwd than
        //    crash, callers can override if they need different
        //    behavior.
        let config_home = overrides
            .config_home
            .or_else(dirs::config_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let cache_home = overrides
            .cache_home
            .or_else(dirs::cache_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let data_home = overrides
            .data_home
            .or_else(dirs::data_dir)
            .unwrap_or_else(|| PathBuf::from("."));

        // 2. Derive app-specific paths.
        let config_dir = config_home.join(app_name);
        let databases_dir = config_dir.join("databases");
        let cache_dir = cache_home.join(app_name);
        let secrets_dir = config_dir.join("secrets");
        let plugins_dir = config_dir.join("plugins");
        let logs_dir = data_home.join(app_name).join("logs");
        let temp_dir = std::env::temp_dir().join(app_name);

        let dirs = Self {
            config_dir,
            databases_dir,
            cache_dir,
            secrets_dir,
            plugins_dir,
            logs_dir,
            temp_dir,
        };

        // 3. Create + chmod each path.
        let paths_to_create = [
            (&dirs.config_dir, "config"),
            (&dirs.databases_dir, "databases"),
            (&dirs.cache_dir, "cache"),
            (&dirs.secrets_dir, "secrets"),
            (&dirs.plugins_dir, "plugins"),
            (&dirs.logs_dir, "logs"),
            (&dirs.temp_dir, "temp"),
        ];

        for (path, label) in paths_to_create {
            #[cfg(windows)]
            {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let reserved = [
                        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                        "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
                        "LPT7", "LPT8", "LPT9",
                    ];
                    if reserved.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
                        return Err(anyhow::anyhow!(
                            "Reserved Windows filename detected: {:?}",
                            stem
                        ));
                    }
                }
            }

            ensure_owner_dir(path)
                .with_context(|| format!("creating {} dir at {:?}", label, path))?;

            // On Windows, names like `CON` resolve to a device, not a
            // directory — create_dir_all might "succeed" without actually
            // making a dir. Belt + suspenders.
            if !path.is_dir() {
                return Err(anyhow::anyhow!(
                    "Path exists but is not a directory (reserved name?): {:?}",
                    path
                ));
            }
        }

        Ok(dirs)
    }
}

#[cfg(test)]
mod tests;
