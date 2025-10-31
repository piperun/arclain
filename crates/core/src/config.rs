use serde::{Deserialize, Serialize};
use std::{fs, path::{PathBuf}, io::Write};
use anyhow::{Result, Context};
use regex::Regex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassRule {
    pub name: String,
    pub pattern: String,   // regex
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
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
    pub fn auto_password_for(&self, filenames: &[String]) -> Option<String> {
        let mut rules: Vec<&PassRule> = self.cfg.pass_rules.iter().filter(|r| r.enabled).collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        for r in rules {
            if let Some(re) = r.to_regex() {
                if filenames.iter().any(|f| re.is_match(f)) {
                    return Some(r.password.clone());
                }
            }
        }
        None
    }
}