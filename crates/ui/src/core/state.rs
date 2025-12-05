use anyhow::Result;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::config::database::{
    get_config, list_pass_rules, open_databases, replace_pass_rules, set_config, ConfigDb,
    ConfigDbs, DbPaths, SecretsDb, SecretsKey,
};
use arclain_core::utilities::ChecksumService;
use arclain_core::{ConfigStore, NavigationState};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info, warn};

pub struct AppState {
    pub cfg: ConfigStore,
    pub backend_selector: BackendSelector,
    pub fallback_backend: SevenZipCli, // Keep for plugin compatibility
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
    // Plugin system
    pub plugin_manager: Option<Arc<Mutex<PluginManager>>>,
    pub plugin_metadata: Option<serde_json::Value>,
    // Game metadata for archive organization (from plugins like DLSite)
    pub current_game_metadata: Option<arclain_core::organization::GameMetadata>,
    pub archive_info: crate::core::operations::archive::ArchiveInfo,
    // Checksum verification service
    pub checksum_service: Option<ChecksumService>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        info!("Initializing application state");
        let cfg = ConfigStore::load("arclain")?;
        let fallback_backend = SevenZipCli::detect(cfg.cfg.sevenzip_path.as_deref())?;
        info!("7-Zip CLI backend initialized as fallback");

        // Create backend selector (defaults to native mode with fallbacks)
        let backend_selector = BackendSelector::new_native();
        info!("Backend selector initialized (native mode with fallbacks)");

        // Build initial state
        let mut me = Self {
            cfg,
            backend_selector,
            fallback_backend,
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
            plugin_manager: None,
            plugin_metadata: None,
            current_game_metadata: None,
            archive_info: crate::core::operations::archive::ArchiveInfo::default(),
            checksum_service: None,
        };

        // Attempt to open DB-backed config + secrets (optional)
        if let Ok(mut paths) = DbPaths::defaults("arclain") {
            // Read overrides from config.sqlite if present
            if let Ok(cfg_db) = ConfigDb::open(&paths.config_db) {
                let cfg_conn = cfg_db.into_sqlite_db();

                // Try to read secrets_db_path
                if let Ok(Some(secrets_path)) =
                    cfg_conn.with_connection(|conn| get_config(conn, "secrets_db_path"))
                {
                    paths.secrets_db = PathBuf::from(secrets_path);
                }

                // Try to read key_file_path
                if let Ok(Some(keyfile_path)) =
                    cfg_conn.with_connection(|conn| get_config(conn, "key_file_path"))
                {
                    me.db_paths = Some(paths.clone());
                    // env var can override later
                    paths.key_file = Some(PathBuf::from(keyfile_path));
                }

                // Try to read encrypted_crc_policy
                if let Ok(Some(policy)) =
                    cfg_conn.with_connection(|conn| get_config(conn, "encrypted_crc_policy"))
                {
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
                    info!(
                        "Master key file not found, generating new key at: {}",
                        key_path.display()
                    );
                    let new_key = SecretsKey::generate();
                    if let Err(e) = new_key.save_to_file(key_path) {
                        warn!("Failed to save generated key file: {}", e);
                    } else {
                        info!("Master key file created successfully");
                    }
                }
            }

            // Open secrets DB if key file provided and valid
            // Open secrets DB if key file provided and valid
            if let Some(ref key_path) = paths.key_file {
                if let Ok(key) = SecretsKey::load_from_file(key_path) {
                    match open_databases(&paths, &key) {
                        Ok(mut dbs) => {
                            // Persist current paths into config DB
                            let _ = dbs.config.with_connection(|conn| {
                                set_config(
                                    conn,
                                    "secrets_db_path",
                                    &paths.secrets_db.to_string_lossy(),
                                )?;
                                set_config(conn, "key_file_path", &key_path.to_string_lossy())
                            });

                            // Migrate plain JSON settings -> config.sqlite if not present
                            let has_sevenzip = matches!(
                                dbs.config
                                    .with_connection(|conn| get_config(conn, "sevenzip_path")),
                                Ok(Some(_))
                            );

                            if !has_sevenzip {
                                if let Some(p) = me.cfg.cfg.sevenzip_path.clone() {
                                    let _ = dbs.config.with_connection(|conn| {
                                        set_config(conn, "sevenzip_path", &p.to_string_lossy())
                                    });
                                }
                            }

                            let has_transfer = matches!(
                                dbs.config
                                    .with_connection(|conn| get_config(conn, "transfer_dir")),
                                Ok(Some(_))
                            );

                            if !has_transfer {
                                if let Some(p) = me.cfg.cfg.transfer_dir.clone() {
                                    let _ = dbs.config.with_connection(|conn| {
                                        set_config(conn, "transfer_dir", &p.to_string_lossy())
                                    });
                                }
                            }

                            // Migrate JSON pass_rules -> secrets DB (first run)
                            let migrated = if let Ok(res) = dbs
                                .config
                                .with_connection(|conn| get_config(conn, "pass_rules_migrated"))
                            {
                                res
                            } else {
                                None
                            };

                            if migrated.as_deref() != Some("1") {
                                if let Ok(existing) = list_pass_rules(&dbs.secrets) {
                                    if existing.is_empty() && !me.cfg.cfg.pass_rules.is_empty() {
                                        if let Err(e) = replace_pass_rules(
                                            &mut dbs.secrets,
                                            &me.cfg.cfg.pass_rules,
                                        ) {
                                            info!("Pass rules migration failed: {}", e);
                                        } else {
                                            let _ = dbs.config.with_connection(|conn| {
                                                set_config(conn, "pass_rules_migrated", "1")
                                            });
                                            info!(
                                                "Migrated {} pass rules into secrets DB",
                                                me.cfg.cfg.pass_rules.len()
                                            );
                                        }
                                    } else if !existing.is_empty() {
                                        let _ = dbs.config.with_connection(|conn| {
                                            set_config(conn, "pass_rules_migrated", "1")
                                        });
                                    }
                                }
                            }

                            // Load pass rules from secrets DB and replace in-memory rules
                            if let Ok(rules) = list_pass_rules(&dbs.secrets) {
                                me.cfg.cfg.pass_rules = rules;
                                info!(
                                    "Loaded {} pass rules from encrypted secrets DB",
                                    me.cfg.cfg.pass_rules.len()
                                );
                            }

                            // Store connections and paths
                            me.db_paths = Some(paths.clone());
                            me.dbs = Some(dbs);

                            // Sync configuration from TOML defaults
                            me.sync_configuration();

                            // Initialize checksum service and recover pending operations
                            let checksum_db_path = paths
                                .config_db
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("checksum.sqlite");
                            match ChecksumService::open(&checksum_db_path) {
                                Ok(service) => {
                                    // Recover any interrupted operations from previous session
                                    match service.recover_pending() {
                                        Ok(actions) => {
                                            for action in &actions {
                                                debug!("Checksum recovery: {:?}", action);
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Failed to recover checksum operations: {}", e);
                                        }
                                    }
                                    me.checksum_service = Some(service);
                                    info!("Checksum verification service initialized");
                                }
                                Err(e) => {
                                    warn!("Failed to initialize checksum service: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to open databases: {}", e);
                            info!("Config DB: {}", paths.config_db.display());
                            info!("Cache DB: {}", paths.cache_db.display());
                            info!("Secrets DB: {}", paths.secrets_db.display());
                            info!("Key file: {}", key_path.display());
                            info!("Falling back to JSON config");
                        }
                    }
                } else {
                    warn!("Invalid key file; falling back to JSON config");
                }
            }
        }

        // Initialize plugin manager with fallback backend for compatibility
        info!("Initializing plugin system");
        let plugins_dir = PathBuf::from("plugins");
        let backend_arc = Arc::new(me.fallback_backend.clone());
        match PluginManager::with_backend(plugins_dir, backend_arc) {
            Ok(mut manager) => {
                // Initialize plugins
                if let Err(e) = manager.init() {
                    warn!("Failed to initialize plugins: {}", e);
                } else {
                    let plugin_count = manager.list_plugins().len();
                    info!("Plugin manager initialized with {} plugins", plugin_count);
                }
                if let Some(ref dbs) = me.dbs {
                    manager.set_metadata_cache(Arc::new(dbs.metadata.clone()));
                }
                me.plugin_manager = Some(Arc::new(Mutex::new(manager)));
            }
            Err(e) => {
                warn!("Failed to initialize plugin manager: {}", e);
                info!("Application will continue without plugin support");
            }
        }

        Ok(me)
    }

    pub fn list_archive(&mut self, path: &Path) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());

        // Select appropriate backend based on file extension
        let backend = self.backend_selector.select(path)?;

        let info = match backend.list(path, None) {
            Ok(info) => {
                debug!("Archive opened without password (may have encrypted files inside)");
                info
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let archive_name = path.to_str();
                let pw = self.cfg.auto_password_for(archive_name, &self.last_entries);
                if let Some(ref password) = pw {
                    info!("Attempting to open archive with auto-detected password");
                    let info = backend.list(path, Some(password))?;
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
            let archive_name = self.current_archive.as_ref().and_then(|p| p.to_str());
            debug!(
                "Attempting auto-password detection for archive: {:?}",
                archive_name
            );
            let detected_pw = self.cfg.auto_password_for(archive_name, &self.last_entries);
            if let Some(ref pwd) = detected_pw {
                info!("Auto-detected password for archive (length: {})", pwd.len());
                self.current_password = Some(pwd.clone());
            } else {
                debug!("No password auto-detected for this archive");
            }
        }
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();

        // Dispatch OnArchiveOpen event to plugins
        self.plugin_metadata = None; // Reset metadata
        if let Some(ref manager_arc) = self.plugin_manager {
            // Update archive context for plugins
            {
                let manager = manager_arc.lock();
                manager.set_archive_context(
                    Some(path.to_string_lossy().to_string()),
                    self.current_password.clone(),
                );
            }

            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().to_string(),
                kind: info.archive_kind,
            };

            // Collect metadata responses from plugins
            let mut manager = manager_arc.lock();
            let responses = manager.dispatch_event(&event);
            let mut combined_metadata = serde_json::Map::new();

            for response in responses {
                if let arclain_plugins::PluginResponse::Metadata { data } = response {
                    if let Some(obj) = data.as_object() {
                        // Merge plugin metadata into combined object
                        for (key, value) in obj {
                            combined_metadata.insert(key.clone(), value.clone());
                        }
                    }
                }
            }

            if !combined_metadata.is_empty() {
                let field_count = combined_metadata.len();
                self.plugin_metadata = Some(serde_json::Value::Object(combined_metadata));
                info!("Collected plugin metadata with {} fields", field_count);
            }
        }

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
        let backend = self.backend_selector.select(path)?;
        let info = backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        self.current_password = Some(password.to_string());

        // Update plugin context
        if let Some(ref manager_arc) = self.plugin_manager {
            let manager = manager_arc.lock();
            manager.set_archive_context(
                Some(path.to_string_lossy().to_string()),
                Some(password.to_string()),
            );
        }

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

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let archive_name = archive.to_str();
        let auto_pw = self.cfg.auto_password_for(archive_name, &self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        let backend = self.backend_selector.select(archive)?;
        backend.read_text_file(archive, path_in_archive, pw)
    }

    pub fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.delete_files(archive, files)
    }

    pub fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_or_update_file_from_str(archive, path_in_archive, content)
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
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        if let Some(ref dbp) = secrets_db_path {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "secrets_db_path", dbp));
        }
        if let Some(ref kfp) = key_file_path {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "key_file_path", kfp));
        }
        if let Some(ref pol) = encrypted_crc_policy {
            let _ = cfg_conn.with_connection(|conn| set_config(conn, "encrypted_crc_policy", pol));
            self.encrypted_crc_policy = pol.clone();
        }

        // Try to open encrypted secrets DB only if we have a key file
        if let Some(kp) = paths.key_file.clone() {
            let key = SecretsKey::load_from_file(&kp)?;
            let dbs = open_databases(&paths, &key)?;
            // Store connections and paths
            self.db_paths = Some(paths.clone());

            // Update plugin manager with new cache
            if let Some(ref manager_arc) = self.plugin_manager {
                manager_arc
                    .lock()
                    .set_metadata_cache(Arc::new(dbs.metadata.clone()));
            }

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
        use std::fs;
        use std::path::PathBuf;

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
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        let _ = cfg_conn
            .with_connection(|conn| set_config(conn, "secrets_db_path", &dst.to_string_lossy()));

        // Update paths and reopen if key is available
        paths.secrets_db = dst;
        self.db_paths = Some(paths.clone());

        if let Some(ref kp) = paths.key_file {
            let key = SecretsKey::load_from_file(kp)?;
            let dbs = open_databases(&paths, &key)?;

            // Update plugin manager with new cache
            if let Some(ref manager_arc) = self.plugin_manager {
                manager_arc
                    .lock()
                    .set_metadata_cache(Arc::new(dbs.metadata.clone()));
            }

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
        let old_key = SecretsKey::load_from_file(&old_key_path)?;
        let new_key = SecretsKey::load_from_file(Path::new(new_key_file_path))?;

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
        let cfg_conn = ConfigDb::open(&paths.config_db)?.into_sqlite_db();
        let _ =
            cfg_conn.with_connection(|conn| set_config(conn, "key_file_path", new_key_file_path));

        // Update paths in memory
        paths.key_file = Some(PathBuf::from(new_key_file_path));
        self.db_paths = Some(paths.clone());

        // Update plugin manager with new cache
        if let Some(ref manager_arc) = self.plugin_manager {
            manager_arc
                .lock()
                .set_metadata_cache(Arc::new(new_dbs.metadata.clone()));
        }

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
            arclain_core::config::database::replace_pass_rules(&dbs.secrets, &rules)?;
            info!(
                "Saved {} password rules to encrypted secrets DB",
                rules.len()
            );
        } else {
            // Fall back to JSON if DB not available
            self.cfg.save()?;
            warn!(
                "Saved {} password rules to JSON config (DB not available)",
                rules.len()
            );
        }

        Ok(())
    }

    pub fn save_password_rule_from_archive(
        &mut self,
        archive_path: &Path,
        password: &str,
    ) -> Result<()> {
        let filename = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if filename.is_empty() {
            return Ok(());
        }

        // Generate a pattern from the filename
        // Strategy: escape special regex chars, then try to make it generic if possible
        // For now, we'll just use the exact filename as the pattern to be safe
        let pattern = regex::escape(filename);

        let new_rule = arclain_core::PassRule {
            name: format!("Auto-saved: {}", filename),
            pattern,
            password: password.to_string(),
            priority: 10, // High priority for specific file matches
            enabled: true,
        };

        let mut rules = self.cfg.cfg.pass_rules.clone();
        // Check if a rule with this pattern already exists, if so update it
        if let Some(existing) = rules.iter_mut().find(|r| r.pattern == new_rule.pattern) {
            existing.password = new_rule.password.clone();
            existing.enabled = true;
        } else {
            rules.push(new_rule);
        }

        self.save_password_rules(rules)
    }

    /// Synchronize configuration (rules, filters) from TOML defaults to DB
    pub fn sync_configuration(&self) {
        if let Some(ref dbs) = self.dbs {
            let config_db = dbs.config.clone();
            // Run sync in background or just block? Startup is fine to block briefly.
            if let Err(e) = arclain_core::config::sync::sync_rules(&config_db) {
                warn!("Failed to sync organization rules: {}", e);
            }
            // Title filters are now initialized via title_filter::init()
            if let Some(ref db_paths) = self.db_paths {
                if let Ok(cfg_db) = arclain_core::config::ConfigDb::open(&db_paths.config_db) {
                    if let Err(e) = arclain_core::utilities::title_filter::init(&cfg_db) {
                        warn!("Failed to initialize title filters: {}", e);
                    }
                }
            }
        }
    }
}
