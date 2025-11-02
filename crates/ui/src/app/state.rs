use anyhow::Result;
use arclain_core::sevenzip::SevenZipCli;
use arclain_core::{ArchiveBackend, ConfigStore, NavigationState};
use arclain_core::config_db::{DbPaths, SecretsKey, ConfigDbs, open_config_db, open_databases, get_config, set_config, list_pass_rules, replace_pass_rules, SecretsDb};
use std::{env, path::{Path, PathBuf}};
use tracing::{debug, info, warn};

pub struct AppState {
    pub cfg: ConfigStore,
    pub backend: SevenZipCli,
    pub last_entries: Vec<String>,
    pub all_entries: Vec<arclain_core::ArchiveEntry>,
    pub navigation: NavigationState,
    pub current_archive: Option<PathBuf>,
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
    pub current_password: Option<String>,
    pub encrypted_crc_policy: String,
    // DB-backed settings and secrets (optional; falls back to JSON if unavailable)
    pub db_paths: Option<DbPaths>,
    pub dbs: Option<ConfigDbs>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        info!("Initializing application state");
        let cfg = ConfigStore::load("arclain")?;
        let backend = SevenZipCli::detect(cfg.cfg.sevenzip_path.as_deref())?;
        info!("7-Zip backend initialized");

        // Build initial state
        let mut me = Self {
            cfg,
            backend,
            last_entries: vec![],
            all_entries: vec![],
            navigation: NavigationState::new(),
            current_archive: None,
            archive_encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
            current_password: None,
            encrypted_crc_policy: "on_open".to_string(),
            db_paths: None,
            dbs: None,
        };

        // Attempt to open DB-backed config + secrets (optional)
        if let Ok(mut paths) = DbPaths::defaults("arclain") {
            // Read overrides from config.sqlite if present
            if let Ok(cfg_conn) = open_config_db(&paths.config_db) {
                if let Ok(Some(secrets_path)) = get_config(&cfg_conn, "secrets_db_path") {
                    paths.secrets_db = PathBuf::from(secrets_path);
                }
                if let Ok(Some(keyfile_path)) = get_config(&cfg_conn, "key_file_path") {
                    me.db_paths = Some(paths.clone());
                    // env var can override later
                    paths.key_file = Some(PathBuf::from(keyfile_path));
                }
                if let Ok(Some(policy)) = get_config(&cfg_conn, "encrypted_crc_policy") {
                    me.encrypted_crc_policy = policy;
                }
            }

            // Environment variable override for key file (optional)
            if let Ok(kf) = env::var("ARCLAIN_KEYFILE") {
                let kf = kf.trim();
                if !kf.is_empty() {
                    paths.key_file = Some(PathBuf::from(kf));
                }
            }

            // Auto-generate key if it doesn't exist yet
            if let Some(ref key_path) = paths.key_file {
                if !key_path.exists() {
                    info!("Master key file not found, generating new key at: {}", key_path.display());
                    let new_key = SecretsKey::generate();
                    if let Err(e) = new_key.save_to_file(key_path) {
                        warn!("Failed to save generated key file: {}", e);
                    } else {
                        info!("Master key file created successfully");
                    }
                }
            }

            // Open secrets DB if key file provided and valid
            if let Some(ref key_path) = paths.key_file {
                if let Ok(key) = SecretsKey::from_file(key_path) {
                    match open_databases(&paths, &key) {
                        Ok(mut dbs) => {
                        // Persist current paths into config DB
                        let _ = set_config(&dbs.config, "secrets_db_path", &paths.secrets_db.to_string_lossy());
                        let _ = set_config(&dbs.config, "key_file_path", &key_path.to_string_lossy());

                        // Migrate plain JSON settings -> config.sqlite if not present
                        if get_config(&dbs.config, "sevenzip_path").unwrap_or(None).is_none() {
                            if let Some(p) = me.cfg.cfg.sevenzip_path.clone() {
                                let _ = set_config(&dbs.config, "sevenzip_path", &p.to_string_lossy());
                            }
                        }
                        if get_config(&dbs.config, "transfer_dir").unwrap_or(None).is_none() {
                            if let Some(p) = me.cfg.cfg.transfer_dir.clone() {
                                let _ = set_config(&dbs.config, "transfer_dir", &p.to_string_lossy());
                            }
                        }

                        // Migrate JSON pass_rules -> secrets DB (first run)
                        let migrated = get_config(&dbs.config, "pass_rules_migrated").unwrap_or(None);
                        if migrated.as_deref() != Some("1") {
                            if let Ok(existing) = list_pass_rules(&dbs.secrets) {
                                if existing.is_empty() && !me.cfg.cfg.pass_rules.is_empty() {
                                    if let Err(e) = replace_pass_rules(&mut dbs.secrets, &me.cfg.cfg.pass_rules) {
                                        info!("Pass rules migration failed: {}", e);
                                    } else {
                                        let _ = set_config(&dbs.config, "pass_rules_migrated", "1");
                                        info!("Migrated {} pass rules into secrets DB", me.cfg.cfg.pass_rules.len());
                                    }
                                } else if !existing.is_empty() {
                                    let _ = set_config(&dbs.config, "pass_rules_migrated", "1");
                                }
                            }
                        }

                        // Load pass rules from secrets DB and replace in-memory rules
                        if let Ok(rules) = list_pass_rules(&dbs.secrets) {
                            me.cfg.cfg.pass_rules = rules;
                            info!("Loaded {} pass rules from encrypted secrets DB", me.cfg.cfg.pass_rules.len());
                        }

                            // Store connections and paths
                            me.db_paths = Some(paths.clone());
                            me.dbs = Some(dbs);
                        }
                        Err(e) => {
                            warn!("Failed to open secrets DB: {}; falling back to JSON config", e);
                            info!("Database path: {}", paths.secrets_db.display());
                            info!("Key file: {}", key_path.display());
                        }
                    }
                } else {
                    warn!("Invalid key file; falling back to JSON config");
                }
            }
        }

        Ok(me)
    }

    pub fn list_archive(&mut self, path: &Path) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        let info = match self.backend.list(path, None) {
            Ok(info) => {
                debug!("Archive opened without password (may have encrypted files inside)");
                info
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let pw = self.cfg.auto_password_for(&self.last_entries);
                if let Some(ref password) = pw {
                    info!("Attempting to open archive with auto-detected password");
                    let info = self.backend.list(path, Some(password))?;
                    self.current_password = Some(password.clone());
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        if self.current_password.is_none() {
            let detected_pw = self.cfg.auto_password_for(&self.last_entries);
            if let Some(pwd) = detected_pw {
                self.current_password = Some(pwd);
            }
        }
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        info!(
            "Archive opened successfully with {} entries",
            self.all_entries.len()
        );
        Ok(self.all_entries.clone())
    }

    pub fn list_with_password(
        &mut self,
        path: &Path,
        password: &str,
    ) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Listing archive with manually provided password");
        let info = self.backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        self.current_password = Some(password.to_string());
        Ok(self.all_entries.clone())
    }

    pub fn navigate_to_folder(&mut self, folder: &str) {
        debug!("Navigating to folder: {}", folder);
        self.navigation.navigate_to(folder);
    }

    pub fn navigate_back(&mut self) {
        debug!("Navigating back from: {}", self.navigation.current_path);
        self.navigation.navigate_back();
    }

    pub fn navigate_forward(&mut self) {
        debug!("Navigating forward from: {}", self.navigation.current_path);
        self.navigation.navigate_forward();
    }

    pub fn navigate_up(&mut self) {
        debug!("Navigating up from: {}", self.navigation.current_path);
        self.navigation.navigate_up();
    }

    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        self.navigation.filter_entries(&self.all_entries)
    }

    pub fn extract_all(&self, archive: &Path, dest: &Path) -> Result<()> {
        info!(
            "Extracting all files from {} to {}",
            archive.display(),
            dest.display()
        );
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend.extract_all(archive, dest, pw)
    }

    pub fn extract_selected(&self, archive: &Path, dest: &Path, files: Vec<String>) -> Result<()> {
        info!("Extracting {} selected files", files.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());

        let full_paths: Vec<String> = if !self.navigation.current_path.is_empty() {
            files
                .iter()
                .map(|f| format!("{}/{}", self.navigation.current_path, f))
                .collect()
        } else {
            files
        };

        self.backend.extract_files(archive, dest, &full_paths, pw)
    }

    pub fn extract_specific(&self, archive: &Path, dest: &Path, full_paths: Vec<String>) -> Result<()> {
        info!("Extracting {} file(s) (exact paths)", full_paths.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        if full_paths.len() > 100 {
            let dir_path = if let Some(first) = full_paths.first() {
                if let Some(idx) = first.rfind('/') {
                    &first[..idx]
                } else {
                    ""
                }
            } else {
                ""
            };
            info!(
                "Too many files ({}), extracting entire directory: {}",
                full_paths.len(),
                if dir_path.is_empty() { "<root>" } else { dir_path }
            );
            self.backend.extract_directory(archive, dest, dir_path, pw)
        } else {
            debug!("Files to extract: {:?}", full_paths);
            self.backend.extract_files(archive, dest, &full_paths, pw)
        }
    }

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        self.backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend.read_text_file(archive, path_in_archive, pw)
    }

    pub fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        self.backend.delete_files(archive, files)
    }

    pub fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        self.backend.add_or_update_file_from_str(archive, path_in_archive, content)
    }

    /// Apply Preferences changes: persist overrides and (re)open SQLCipher DBs.
    /// - key_file_path: Optional path to key file (32-byte raw/hex/base64)
    /// - secrets_db_path: Optional path to pass.sqlite (SQLCipher)
    /// - encrypted_crc_policy: Optional policy string: "lazy_prompt" | "auto_prompt" | "per_file"
    pub fn apply_preferences(
        &mut self,
        key_file_path: Option<String>,
        secrets_db_path: Option<String>,
        encrypted_crc_policy: Option<String>,
    ) -> Result<()> {
        // Start from existing paths or defaults
        let mut paths = if let Some(p) = self.db_paths.clone() {
            p
        } else {
            DbPaths::defaults("arclain")?
        };

        if let Some(ref dbp) = secrets_db_path {
            paths.secrets_db = PathBuf::from(dbp);
        }
        if let Some(ref kfp) = key_file_path {
            paths.key_file = Some(PathBuf::from(kfp));
        }

        // Always persist overrides into plain config.sqlite
        let cfg_conn = open_config_db(&paths.config_db)?;
        if let Some(ref dbp) = secrets_db_path {
            let _ = set_config(&cfg_conn, "secrets_db_path", dbp);
        }
        if let Some(ref kfp) = key_file_path {
            let _ = set_config(&cfg_conn, "key_file_path", kfp);
        }
        if let Some(ref pol) = encrypted_crc_policy {
            let _ = set_config(&cfg_conn, "encrypted_crc_policy", pol);
            self.encrypted_crc_policy = pol.clone();
        }

        // Try to open encrypted secrets DB only if we have a key file
        if let Some(kp) = paths.key_file.clone() {
            let key = SecretsKey::from_file(&kp)?;
            let dbs = open_databases(&paths, &key)?;
            // Store connections and paths
            self.db_paths = Some(paths.clone());
            self.dbs = Some(dbs);

            // Load pass rules from secrets DB
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                    self.cfg.cfg.pass_rules = rules;
                }
            }
        } else {
            // No key provided, still keep updated paths
            self.db_paths = Some(paths.clone());
        }

        Ok(())
    }

    pub fn move_vault(&mut self, dest_path: &str) -> Result<()> {
        use std::path::PathBuf;
        use std::fs;
        
        // Establish paths
        let mut paths = if let Some(p) = self.db_paths.clone() {
            p
        } else {
            DbPaths::defaults("arclain")?
        };

        // Close existing DBs to avoid file locks
        if let Some(_) = self.dbs.take() {
            // dropped
        }

        let src = paths.secrets_db.clone();
        let dst = PathBuf::from(dest_path);
        
        // Simple file copy for redb
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&src, &dst)?;
        
        // Set secure permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&dst, perms)?;
        }

        // Persist new path in config DB
        let cfg_conn = open_config_db(&paths.config_db)?;
        let _ = set_config(&cfg_conn, "secrets_db_path", &dst.to_string_lossy());

        // Update paths and reopen if key is available
        paths.secrets_db = dst;
        self.db_paths = Some(paths.clone());

        if let Some(ref kp) = paths.key_file {
            let key = SecretsKey::from_file(kp)?;
            let dbs = open_databases(&paths, &key)?;
            self.dbs = Some(dbs);

            // Reload pass rules
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                    self.cfg.cfg.pass_rules = rules;
                }
            }
        }
        Ok(())
    }

    pub fn rekey_vault(&mut self, new_key_file_path: &str) -> Result<()> {
        use std::path::PathBuf;
        
        // Resolve paths
        let mut paths = if let Some(p) = self.db_paths.clone() {
            p
        } else {
            DbPaths::defaults("arclain")?
        };

        // Ensure current key exists
        let old_key_path = paths
            .key_file
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No current key file configured"))?;
        let old_key = SecretsKey::from_file(&old_key_path)?;
        let new_key = SecretsKey::from_file(Path::new(new_key_file_path))?;

        // For redb, we need to:
        // 1. Read all data with old key
        // 2. Create new database with new key
        // 3. Write all data with new key
        
        // Read all rules with old key
        let rules = if let Some(ref dbs) = self.dbs {
            list_pass_rules(&dbs.secrets)?
        } else {
            let old_db = SecretsDb::open(&paths.secrets_db, &old_key.as_bytes())?;
            list_pass_rules(&old_db)?
        };
        
        // Close old database
        if let Some(_) = self.dbs.take() {
            // dropped
        }
        
        // Create backup
        let backup_path = paths.secrets_db.with_extension("redb.backup");
        std::fs::copy(&paths.secrets_db, &backup_path)?;
        
        // Remove old database and create new one with new key
        std::fs::remove_file(&paths.secrets_db)?;
        let new_dbs = open_databases(&paths, &new_key)?;
        
        // Write rules to new database
        replace_pass_rules(&new_dbs.secrets, &rules)?;
        
        // Persist new key file path
        let cfg_conn = open_config_db(&paths.config_db)?;
        let _ = set_config(&cfg_conn, "key_file_path", new_key_file_path);

        // Update paths in memory
        paths.key_file = Some(PathBuf::from(new_key_file_path));
        self.db_paths = Some(paths.clone());
        self.dbs = Some(new_dbs);

        // Reload pass rules (should be the same, but for consistency)
        if let Some(ref dbs_ref) = self.dbs {
            if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                self.cfg.cfg.pass_rules = rules;
            }
        }

        Ok(())
    }

    /// Save password rules to the encrypted secrets database
    pub fn save_password_rules(&mut self, rules: Vec<arclain_core::PassRule>) -> Result<()> {
        // Update in-memory cache
        self.cfg.cfg.pass_rules = rules.clone();
        
        // Persist to secrets DB if available
        if let Some(ref dbs) = self.dbs {
            replace_pass_rules(&dbs.secrets, &rules)?;
            info!("Saved {} password rules to encrypted secrets DB", rules.len());
        } else {
            // Fall back to JSON if DB not available
            self.cfg.save()?;
            warn!("Saved {} password rules to JSON config (DB not available)", rules.len());
        }
        
        Ok(())
    }
}