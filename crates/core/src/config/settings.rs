use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

// Re-export PassRule from password_matcher for backwards compatibility
pub use crate::utilities::password_matcher::PassRule;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub sevenzip_path: Option<PathBuf>,
    pub transfer_dir: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
    pub pass_rules: Vec<PassRule>,
    /// Backend selection: "native" (use native backends where possible) or "cli" (always use 7z CLI)
    #[serde(default = "default_backend_mode")]
    pub backend_mode: String,
    /// Whether to open nested archives in a new tab (true) or replace current view (false)
    #[serde(default)]
    pub open_nested_in_new_tab: bool,
}

fn default_backend_mode() -> String {
    "native".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sevenzip_path: None,
            transfer_dir: None,
            temp_dir: None,
            pass_rules: vec![],
            backend_mode: default_backend_mode(),
            open_nested_in_new_tab: false, // Default: replace current view
        }
    }
}

pub struct ConfigStore {
    path: PathBuf,
    pub cfg: Config,
}

impl ConfigStore {
    pub fn load(app_name: &str) -> Result<Self> {
        let proj = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = proj.join(app_name);
        fs::create_dir_all(&dir).ok();
        let path = dir.join("config.json");
        let cfg = if path.exists() {
            let s = fs::read_to_string(&path).context("reading config")?;
            serde_json::from_str(&s).context("parsing config")?
        } else {
            Config::default()
        };
        Ok(Self { path, cfg })
    }

    pub fn save(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.cfg)?;
        let mut f = fs::File::create(&self.path)?;
        f.write_all(data.as_bytes())?;
        Ok(())
    }

    // Returns the first matching password by descending priority
    // Matches against both the archive filename and file entries inside
    pub fn auto_password_for(
        &self,
        archive_path: Option<&str>,
        filenames: &[String],
    ) -> Option<String> {
        let mut rules: Vec<&PassRule> = self.cfg.pass_rules.iter().filter(|r| r.enabled).collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        for r in rules {
            if let Some(re) = r.to_regex() {
                // First check archive filename if provided
                if let Some(archive) = archive_path {
                    // Extract just the filename from the full path
                    let archive_name = std::path::Path::new(archive)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(archive);

                    if re.is_match(archive_name) {
                        return Some(r.password.clone());
                    }
                }

                // Also check internal file paths for backwards compatibility
                if filenames.iter().any(|f| re.is_match(f)) {
                    return Some(r.password.clone());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
