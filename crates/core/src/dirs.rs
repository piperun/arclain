use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Centralized directory management for the application.
/// Ensures all infrastructure directories exist at startup.
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

/// Optional overrides for base paths (useful for testing or portable mode)
#[derive(Debug, Clone, Default)]
pub struct PathOverrides {
    pub config_home: Option<PathBuf>,
    pub cache_home: Option<PathBuf>,
    pub data_home: Option<PathBuf>,
}

impl AppDirectories {
    /// Initialize application directories.
    ///
    /// This function:
    /// 1. Calculates standard paths based on OS conventions (or overrides).
    /// 2. creating the directories if they don't exist.
    /// 3. Returns the paths struct.
    ///
    /// This should be called ONCE at application startup.
    pub fn init(app_name: &str, overrides: Option<PathOverrides>) -> Result<Self> {
        let overrides = overrides.unwrap_or_default();

        // 1. Calculate Base Paths
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

        // 2. Derive Application Paths
        // Windows: %APPDATA%/app_name/
        // Linux: ~/.config/app_name/
        let config_dir = config_home.join(app_name);
        let databases_dir = config_dir.join("databases");

        // Windows: %LOCALAPPDATA%/app_name/cache/ (or similar, depends on dirs crate)
        // Linux: ~/.cache/app_name/
        let cache_dir = cache_home.join(app_name);

        // Secrets: %APPDATA%/app_name/secrets/
        let secrets_dir = config_dir.join("secrets");

        // Plugins: %APPDATA%/app_name/plugins/
        let plugins_dir = config_dir.join("plugins");

        // Logs: %APPDATA%/app_name/logs/ (or use data dir on linux?)
        // For simplicity and ease of access, standardizing on config/logs or data/logs.
        // Let's use config/logs for now as it's easier for users to find on Windows,
        // or data_home if we want to be strict XDG.
        // Let's stick to config_dir/logs to match previous behavior if any, or separate if clean.
        // Actually, let's use data_home/app_name/logs to be cleaner?
        // User asked for "root" creation. Let's put logs in data_dir to keep config clean?
        // Let's check where logs are currently... `logging.rs` uses `dirs::data_local_dir()`.
        let logs_dir = data_home.join(app_name).join("logs");

        // Temp: System temp / app_name
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

        // 3. Create Directories (The centralized side-effect)
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

            fs::create_dir_all(path)
                .with_context(|| format!("creating {} dir: {:?}", label, path))?;

            // Validation: Ensure it is actually a directory and not a device/file
            // On Windows, 'CON' exists but is not a directory.
            if !path.is_dir() {
                return Err(anyhow::anyhow!(
                    "Path exists but is not a directory (reserved name?): {:?}",
                    path
                ));
            }
        }

        // Secure permissions for secrets on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o700);
            if let Err(e) = fs::set_permissions(&dirs.secrets_dir, perms) {
                // Log warning but don't fail hard if not owner (e.g. shared FS)
                // actually, for secrets, maybe we should fail?
                // Context("setting permissions")?
                tracing::warn!("Failed to secure secrets dir permissions: {}", e);
            }
        }

        Ok(dirs)
    }
}

#[cfg(test)]
mod tests;
