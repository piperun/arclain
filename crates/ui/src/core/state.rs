use anyhow::Result;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::config::database::{
    get_config, list_pass_rules, open_databases, replace_pass_rules, set_config, ConfigDb,
    ConfigDbs, DbPaths, SecretsDb, SecretsKey,
};
use arclain_core::utilities::{auto_password_for, ChecksumService, PassRule};
use arclain_core::NavigationState;
use arclain_data::{ContentCache, ResourceConfig, ResourceManager};
use arclain_db::UserConfig;
use arclain_http::features::whitelist::DomainWhitelist;
use arclain_http::AsyncHttpClient;
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use parking_lot::RwLock;
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info, warn};

pub struct AppState {
    /// User configuration loaded from database
    pub user_config: UserConfig,
    /// Password rules loaded from encrypted secrets DB
    pub pass_rules: Vec<PassRule>,
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
    pub current_game_metadata: Option<arclain_core::features::organization::GameMetadata>,
    pub archive_info: crate::core::operations::archive::ArchiveInfo,
    // Checksum verification service
    pub checksum_service: Option<ChecksumService>,
    // Content cache for plugin images
    pub content_cache: Option<Arc<ContentCache>>,
    // UI preferences
    pub ui_preferences: UiPreferences,
    // Toolbar config items (loaded from DB)
    pub toolbar_items: Vec<arclain_db::UiItem>,
    pub info_panel_items: Vec<arclain_db::UiItem>,
    // Async Runtime and HTTP
    #[allow(dead_code)] // Runtime is kept alive by AppState
    pub tokio_runtime: tokio::runtime::Runtime,
    pub async_http_client: Option<Arc<AsyncHttpClient>>,
    pub resource_manager: Option<Arc<ResourceManager>>,
    #[allow(dead_code)] // Used in future UI settings
    pub domain_whitelist: Arc<RwLock<DomainWhitelist>>,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}

impl AppState {
    pub fn new() -> Result<Self> {
        info!("Initializing application state");

        // Initialize Tokio runtime
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let domain_whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));

        // Initialize AsyncHttpClient
        let async_http_client = Arc::new(AsyncHttpClient::new(
            runtime.handle().clone(),
            domain_whitelist.clone(),
        ));

        // Load user config from database
        let db_paths = DbPaths::defaults("arclain")?;
        let user_config = if let Ok(cfg_db) = ConfigDb::open(&db_paths.config_db) {
            let cfg_conn = cfg_db.into_sqlite_db();
            cfg_conn
                .with_connection(|conn| {
                    UserConfig::ensure_table(conn)?;
                    Ok(UserConfig::load(conn)?.unwrap_or_default())
                })
                .unwrap_or_default()
        } else {
            UserConfig::default()
        };

        // Initialize 7-Zip backend with path from config
        let sevenzip_path = user_config.sevenzip_path.as_ref().map(PathBuf::from);
        let fallback_backend = SevenZipCli::detect(sevenzip_path.as_deref())?;
        info!("7-Zip CLI backend initialized as fallback");

        // Create backend selector (defaults to native mode with fallbacks)
        let backend_selector = BackendSelector::new_native();
        info!("Backend selector initialized (native mode with fallbacks)");

        // Build initial state
        let mut me = Self {
            user_config,
            pass_rules: vec![],
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
            db_paths: Some(db_paths.clone()),
            dbs: None,
            plugin_manager: None,
            plugin_metadata: None,
            current_game_metadata: None,
            archive_info: crate::core::operations::archive::ArchiveInfo::default(),
            checksum_service: None,
            content_cache: None,
            ui_preferences: UiPreferences::default(),
            toolbar_items: vec![],
            info_panel_items: vec![],
            tokio_runtime: runtime,
            async_http_client: Some(async_http_client),
            resource_manager: None,
            domain_whitelist,
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
                        Ok(dbs) => {
                            // Persist current paths into config DB
                            let _ = dbs.config.with_connection(|conn| {
                                set_config(
                                    conn,
                                    "secrets_db_path",
                                    &paths.secrets_db.to_string_lossy(),
                                )?;
                                set_config(conn, "key_file_path", &key_path.to_string_lossy())
                            });

                            // Note: Organization rules are seeded via sync_configuration() below

                            // Migrate plain JSON settings -> config.sqlite if not present
                            // Load pass rules from secrets DB
                            if let Ok(rules) = list_pass_rules(&dbs.secrets) {
                                me.pass_rules = rules;
                                info!(
                                    "Loaded {} pass rules from encrypted secrets DB",
                                    me.pass_rules.len()
                                );
                            }

                            // Store connections and paths
                            me.db_paths = Some(paths.clone());
                            me.dbs = Some(dbs);

                            // Sync configuration from TOML defaults
                            me.sync_configuration();

                            // Load UI items for config-driven rendering
                            if let Ok(dbs) = me.dbs.as_ref().ok_or(anyhow::anyhow!("No DBs")) {
                                let _ = dbs.config.with_connection(|conn| {
                                    if let Ok(items) = arclain_db::list_items_by_region(
                                        conn,
                                        arclain_db::UiRegion::Toolbar,
                                    ) {
                                        me.toolbar_items = items;
                                    }
                                    if let Ok(items) = arclain_db::list_items_by_region(
                                        conn,
                                        arclain_db::UiRegion::InfoPanel,
                                    ) {
                                        me.info_panel_items = items;
                                    }
                                    Ok::<(), anyhow::Error>(())
                                });
                            }

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

                            // Initialize content cache for plugin images
                            let cache_base_dir = paths
                                .config_db
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join("cache");
                            let cache_index_db_path = cache_base_dir.join("cache_index.sqlite");

                            // Create index DB for cache
                            if let Ok(cache_db) = arclain_db::SqliteDb::open(&cache_index_db_path) {
                                match ContentCache::new(cache_base_dir.clone(), cache_db) {
                                    Ok(cache) => {
                                        let cache_arc = Arc::new(cache);
                                        me.content_cache = Some(cache_arc.clone());
                                        info!("Content cache initialized");

                                        // Initialize Resource Manager
                                        let res_config = ResourceConfig {
                                            fallback_dir: Some(cache_base_dir.join("resources")),
                                            ..Default::default()
                                        };
                                        let resource_manager =
                                            Arc::new(ResourceManager::new(cache_arc, res_config));
                                        me.resource_manager = Some(resource_manager);
                                        info!("Resource manager initialized");
                                    }
                                    Err(e) => {
                                        warn!("Failed to initialize content cache: {}", e);
                                    }
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
        let settings = me.user_config.get_all_plugin_settings();
        match PluginManager::with_backend(plugins_dir, backend_arc, settings) {
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
                    // Also set cache_db for new ProductMetadata table
                    manager.set_cache_db(Arc::new(dbs.metadata.db().clone()));
                }
                if let Some(ref cache) = me.content_cache {
                    manager.set_content_cache(cache.clone());
                }
                if let Some(ref ref_manager) = me.resource_manager {
                    manager.set_resource_manager(ref_manager.clone());
                }
                if let Some(ref client) = me.async_http_client {
                    manager.set_async_http_client(client.clone());
                }
                me.plugin_manager = Some(Arc::new(Mutex::new(manager)));

                // Sync plugin UI items to info_panel_items
                if let Some(ref manager_arc) = me.plugin_manager {
                    let manager = manager_arc.lock();
                    for plugin in manager.list_plugins().iter().filter(|p| p.enabled) {
                        let plugin_id = &plugin.id;
                        info!(
                            "Checking plugin '{}' (id: {}) for Panel UI",
                            plugin.manifest.plugin.name, plugin_id
                        );
                        // Check if plugin provides Panel UI
                        let has_panel = manager
                            .with_plugin_instance(plugin_id, |instance| {
                                let result = instance.get_ui_layout(
                                    arclain_plugins::types::PluginExtensionPoint::Panel,
                                );
                                info!(
                                    "  get_ui_layout result: {:?}",
                                    result.as_ref().map(|e| e.len())
                                );
                                result.map(|e| !e.is_empty()).unwrap_or(false)
                            })
                            .unwrap_or(false);
                        info!("  has_panel: {}", has_panel);

                        if has_panel {
                            // Check if already in info_panel_items
                            let exists = me.info_panel_items.iter().any(|item| {
                                item.action_type == arclain_db::ActionType::Plugin
                                    && item.action_data.as_ref() == Some(plugin_id)
                            });

                            if !exists {
                                let max_sort = me
                                    .info_panel_items
                                    .iter()
                                    .map(|i| i.sort_order)
                                    .max()
                                    .unwrap_or(0);
                                me.info_panel_items.push(arclain_db::UiItem {
                                    id: format!("plugin_{}", plugin_id),
                                    region: arclain_db::UiRegion::InfoPanel,
                                    group_id: Some("plugins".to_string()),
                                    label: plugin.manifest.plugin.name.clone(),
                                    icon: Some("PUZZLE_PIECE".to_string()),
                                    action_type: arclain_db::ActionType::Plugin,
                                    action_data: Some(plugin_id.clone()),
                                    visible: true,
                                    sort_order: max_sort + 1,
                                    display_mode: arclain_db::DisplayMode::IconAndText,
                                });
                                info!(
                                    "Added plugin '{}' to info panel",
                                    plugin.manifest.plugin.name
                                );
                            }
                        }

                        // Also check for PluginButton (toolbar)
                        let has_button = manager
                            .with_plugin_instance(plugin_id, |instance| {
                                instance
                                    .get_ui_layout(
                                        arclain_plugins::types::PluginExtensionPoint::PluginButton,
                                    )
                                    .map(|e| !e.is_empty())
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false);

                        if has_button {
                            let exists = me.toolbar_items.iter().any(|item| {
                                item.action_type == arclain_db::ActionType::Plugin
                                    && item.action_data.as_ref() == Some(plugin_id)
                            });

                            if !exists {
                                let max_sort = me
                                    .toolbar_items
                                    .iter()
                                    .map(|i| i.sort_order)
                                    .max()
                                    .unwrap_or(0);
                                me.toolbar_items.push(arclain_db::UiItem {
                                    id: format!("toolbar_plugin_{}", plugin_id),
                                    region: arclain_db::UiRegion::Toolbar,
                                    group_id: Some("plugins".to_string()),
                                    label: plugin.manifest.plugin.name.clone(),
                                    icon: Some("PUZZLE_PIECE".to_string()),
                                    action_type: arclain_db::ActionType::Plugin,
                                    action_data: Some(plugin_id.clone()),
                                    visible: true,
                                    sort_order: max_sort + 1,
                                    display_mode: arclain_db::DisplayMode::IconOnly,
                                });
                                info!("Added plugin '{}' to toolbar", plugin.manifest.plugin.name);
                            }
                        }
                    }
                }
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
                if info.headers_encrypted {
                    debug!("Archive has encrypted headers, trying auto-password");
                    let archive_name = path.to_str();
                    let pw = auto_password_for(&self.pass_rules, archive_name, &vec![]); // last_entries is empty here anyway
                    if let Some(ref password) = pw {
                        info!("Attempting to open encrypted archive with auto-detected password");
                        match backend.list(path, Some(password)) {
                            Ok(new_info) => {
                                self.current_password = Some(password.clone());
                                new_info
                            }
                            Err(e) => {
                                warn!("Failed to open archive with auto-detected password: {}", e);
                                info // Return original encrypted info so UI shows unlock screen
                            }
                        }
                    } else {
                        debug!("No auto-password found for encrypted archive");
                        info
                    }
                } else {
                    debug!("Archive opened without password");
                    info
                }
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let archive_name = path.to_str();
                let pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
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
        self.all_entries = info.entries.clone();
        // IMPORTANT: Set current_archive BEFORE password detection so archive name matching works
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();

        // Now attempt password detection with correct archive context
        if self.current_password.is_none() {
            let archive_name = self.current_archive.as_ref().and_then(|p| p.to_str());
            debug!(
                "Attempting auto-password detection for archive: {:?}",
                archive_name
            );
            let detected_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
            if let Some(ref pwd) = detected_pw {
                info!("Auto-detected password for archive (length: {})", pwd.len());
                self.current_password = Some(pwd.clone());
            } else if info.encrypted {
                warn!("Archive is encrypted but no password was auto-detected from rules");
            } else {
                debug!("No password needed - archive is not encrypted");
            }
        } else {
            info!(
                "Password already set (length: {})",
                self.current_password.as_ref().map(|p| p.len()).unwrap_or(0)
            );
        }

        // Dispatch OnArchiveOpen event to plugins
        self.plugin_metadata = None; // Reset metadata
        if let Some(ref manager_arc) = self.plugin_manager {
            // Update archive context for plugins
            {
                let mut manager = manager_arc.lock();
                manager.set_archive_context(
                    Some(path.to_string_lossy().to_string()),
                    self.current_password.clone(),
                );
            }

            // Async dispatch to prevent blocking UI
            // Metadata will be populated later via emit_metadata
            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().to_string(),
                kind: info.archive_kind,
            };

            let manager = manager_arc.lock();
            manager.dispatch_event_async(event);
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
            let mut manager = manager_arc.lock();
            manager.set_archive_context(
                Some(path.to_string_lossy().to_string()),
                Some(password.to_string()),
            );
        }

        Ok(self.all_entries.clone())
    }

    pub fn navigate_to_folder(&mut self, folder: &str) {
        debug!("Navigating to folder (relative): {}", folder);
        self.navigation.navigate_to(folder);
    }

    pub fn navigate_to_path(&mut self, path: &str) {
        debug!("Navigating to path (absolute): {}", path);
        self.navigation.navigate_to_absolute(path);
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
        let auto_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
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
                manager_arc
                    .lock()
                    .set_cache_db(Arc::new(dbs.metadata.db().clone()));
            }

            self.dbs = Some(dbs);

            // Load pass rules from secrets DB
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                    self.pass_rules = rules;
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
                manager_arc
                    .lock()
                    .set_cache_db(Arc::new(dbs.metadata.db().clone()));
            }

            self.dbs = Some(dbs);

            // Reload pass rules
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                    self.pass_rules = rules;
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
            manager_arc
                .lock()
                .set_cache_db(Arc::new(new_dbs.metadata.db().clone()));
        }

        self.dbs = Some(new_dbs);

        // Reload pass rules (should be the same, but for consistency)
        if let Some(ref dbs_ref) = self.dbs {
            if let Ok(rules) = list_pass_rules(&dbs_ref.secrets) {
                self.pass_rules = rules;
            }
        }

        Ok(())
    }

    /// Save password rules to the encrypted secrets database
    pub fn save_password_rules(&mut self, rules: Vec<PassRule>) -> Result<()> {
        // Update in-memory cache
        self.pass_rules = rules.clone();

        // Persist to secrets DB if available
        if let Some(ref dbs) = self.dbs {
            arclain_core::config::database::replace_pass_rules(&dbs.secrets, &rules)?;
            info!(
                "Saved {} password rules to encrypted secrets DB",
                rules.len()
            );
        } else {
            // No DB available - can't save
            warn!("Cannot save password rules - DB not available (rules updated in memory only)",);
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

        let mut rules = self.pass_rules.clone();
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

    /// Refresh UI configuration (toolbar/info panel items) from DB
    pub fn reload_ui_config(&mut self) {
        if let Some(ref dbs) = self.dbs {
            let _ = dbs.config.with_connection(|conn| {
                if let Ok(items) =
                    arclain_db::list_items_by_region(conn, arclain_db::UiRegion::Toolbar)
                {
                    self.toolbar_items = items;
                }
                if let Ok(items) =
                    arclain_db::list_items_by_region(conn, arclain_db::UiRegion::InfoPanel)
                {
                    self.info_panel_items = items;
                }
                Ok::<(), anyhow::Error>(())
            });
        }
    }
}
