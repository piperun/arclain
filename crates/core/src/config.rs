use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct PassRule {
    pub name: String,
    pub pattern: String, // regex
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

// Custom Debug implementation to avoid logging passwords
impl std::fmt::Debug for PassRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassRule")
            .field("name", &self.name)
            .field("pattern", &self.pattern)
            .field("password", &"[REDACTED]")
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl PassRule {
    pub fn to_regex(&self) -> Option<Regex> {
        Regex::new(&self.pattern).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub sevenzip_path: Option<PathBuf>,
    pub transfer_dir: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
    pub pass_rules: Vec<PassRule>,
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
            Config {
                sevenzip_path: None,
                transfer_dir: None,
                temp_dir: None,
                pass_rules: vec![],
            }
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
