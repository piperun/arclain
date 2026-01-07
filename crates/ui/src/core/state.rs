use anyhow::Result;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::services::Services as CoreServices;
use arclain_core::services::{ConfigService, SecretsService};
use arclain_core::utilities::{auto_password_for, ChecksumService, PassRule};
use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion, UserConfig};
use arclain_data::{ContentCache, ResourceConfig, ResourceManager};
use arclain_db::{
    open_databases, set_config, ConfigDb, ConfigDbs, DbPassRule, DbPaths, SecretsKey,
};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info, warn};

use super::signals::AppSignals;

pub struct AppState {
    /// User configuration loaded from database
    pub user_config: UserConfig,
    /// Password rules loaded from encrypted secrets DB
    pub pass_rules: Vec<PassRule>,
    pub backend_selector: BackendSelector,
    pub fallback_backend: SevenZipCli, // Keep for plugin compatibility
    pub last_entries: Vec<String>,

    pub encrypted_crc_policy: String,
    // DB-backed settings and secrets (optional; falls back to JSON if unavailable)
    pub db_paths: Option<DbPaths>,
    pub dbs: Option<ConfigDbs>,
    // Plugin system - event sender stays for dispatch, manager moved to Services
    /// Event sender for non-blocking plugin dispatch (no mutex lock needed)
    pub plugin_event_sender: Option<std::sync::mpsc::Sender<arclain_plugins::PluginEvent>>,
    /// Pending plugin event to dispatch after UI is ready.
    /// This is set when an archive opens and cleared after the UI renders
    /// and dispatches the event.
    pub pending_plugin_event: Option<arclain_plugins::PluginEvent>,
    /// Reactive signals for async state updates
    pub signals: AppSignals,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}

impl AppState {
    pub fn new() -> Result<(Self, crate::core::services::Services)> {
        info!("Initializing application state");

        // Initialize Tokio runtime
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        // Local variables for services (will be moved to Services struct at the end)
        let mut plugin_manager: Option<Arc<Mutex<PluginManager>>> = None;
        let mut checksum_service: Option<Arc<ChecksumService>> = None;
        let mut content_cache: Option<Arc<ContentCache>> = None;
        let mut resource_manager: Option<Arc<ResourceManager>> = None;

        // Load startup config using ConfigService helper
        // Initialize infrastructure (creates all dirs)
        let app_dirs = arclain_core::dirs::AppDirectories::init("arclain", None)?;

        // Construct DB paths from initialized directories
        let db_paths = DbPaths {
            config_db: app_dirs.databases_dir.join("config.sqlite"),
            cache_db: app_dirs.databases_dir.join("metadata.sqlite"),
            secrets_db: app_dirs.secrets_dir.join("pass.redb"),
            key_file: Some(app_dirs.secrets_dir.join("master.key")),
        };
        let (secrets_path, key_path, crc_policy) =
            ConfigService::load_startup_config(&db_paths.config_db).unwrap_or((None, None, None));

        let user_config =
            if let Ok(cfg_db) = arclain_core::config::ConfigDb::open(&db_paths.config_db) {
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

            encrypted_crc_policy: crc_policy.unwrap_or_else(|| "on_open".to_string()),
            db_paths: Some(db_paths.clone()),
            dbs: None,
            plugin_event_sender: None,
            pending_plugin_event: None,
            signals: AppSignals::new(),
        };

        me.signals.user_config.set(me.user_config.clone());

        // Update paths from config if present
        let mut current_paths = db_paths.clone();
        if let Some(sp) = secrets_path {
            current_paths.secrets_db = sp;
        }
        if let Some(kp) = key_path {
            current_paths.key_file = Some(kp);
        } else if let Ok(kf) = env::var("ARCLAIN_KEYFILE") {
            if !kf.trim().is_empty() {
                current_paths.key_file = Some(PathBuf::from(kf.trim()));
            }
        }

        // Auto-generate key logic
        if let Some(ref key_path) = current_paths.key_file {
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

        // Initialize Services Manager
        let mut services = CoreServices::new(Arc::new(runtime));

        // Initialize plugin proxy map on the SERVICES client (used by DataService)
        // Note: The local async_http_client variable created earlier is disjoint from Services
        services
            .async_http_client
            .update_plugin_proxy_map(me.user_config.get_plugin_proxy_settings());
        info!("Initialized HTTP client proxy settings");

        // Open Databases and Init Services
        if let Some(ref key_path) = current_paths.key_file {
            if let Ok(key) = SecretsKey::load_from_file(key_path) {
                match open_databases(&current_paths, &key) {
                    Ok(dbs) => {
                        // Initialize Core Services
                        if let Err(e) = services.init_db_services(&dbs, &current_paths) {
                            warn!("Failed to initialize DB services: {}", e);
                        } else {
                            // Initialize Content Cache using service from core
                            if let Some(cache_svc) = services.cache_service.clone() {
                                let cache_dir = services.cache_dir.clone();
                                // Ensure cache directory exists
                                if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                                    warn!("Failed to create cache directory: {}", e);
                                }

                                if let Ok(cache) = ContentCache::new(cache_dir, cache_svc) {
                                    content_cache = Some(Arc::new(cache));
                                    info!("Content cache initialized via Services");
                                }
                            }

                            // Initialize ResourceManager
                            if let Some(ref cache) = content_cache {
                                let res_config = ResourceConfig {
                                    fallback_dir: Some(services.cache_dir.join("resources")),
                                    ..Default::default()
                                };
                                resource_manager =
                                    Some(Arc::new(ResourceManager::new(cache.clone(), res_config)));
                            }

                            // Load pass rules and map to Core PassRule
                            if let Ok(rules) = dbs.secrets.list_pass_rules() {
                                me.pass_rules = rules
                                    .into_iter()
                                    .map(|r| PassRule {
                                        name: r.name,
                                        pattern: r.pattern,
                                        password: r.password,
                                        priority: r.priority,
                                        enabled: r.enabled,
                                    })
                                    .collect();
                            }
                        }

                        me.dbs = Some(dbs);
                        me.db_paths = Some(current_paths.clone());
                        me.sync_configuration();
                    }
                    Err(e) => {
                        warn!("Failed to open databases: {}", e);
                    }
                }
            }
        }

        // Initialize checksum service separately (using path from manager/paths)
        let checksum_db_path = current_paths
            .config_db
            .parent()
            .unwrap_or(Path::new("."))
            .join("checksum.sqlite");
        match ChecksumService::open(&checksum_db_path) {
            Ok(svc) => {
                let _ = svc.recover_pending();
                checksum_service = Some(Arc::new(svc));
            }
            Err(e) => warn!("Failed to init checksum service: {}", e),
        }
        services.checksum_service = checksum_service.clone();
        services.checksum_service = checksum_service.clone();

        // Assign Plugin Manager (which stays in AppState mostly)
        // ... (preserving plugin manager init logic as much as possible, but attaching services)
        // Re-using existing plugin init logic but updating where it gets services from

        info!("Initializing plugin system");
        let plugins_dir = PathBuf::from("plugins");
        let backend_arc = Arc::new(me.fallback_backend.clone());
        let settings = me.user_config.get_all_plugin_settings();
        match PluginManager::with_backend(plugins_dir, backend_arc, settings) {
            Ok(mut manager) => {
                manager.init().ok(); // Ignore error
                                     // Inject services
                if let Some(lib_svc) = services.library_service.clone() {
                    manager.set_library_service(lib_svc);
                }
                if let Some(ref c) = content_cache {
                    manager.set_content_cache(c.clone());
                }
                if let Some(ref r) = resource_manager {
                    manager.set_resource_manager(r.clone());
                }
                manager.set_async_http_client(services.async_http_client.clone());
                manager.set_metadata_signal(me.signals.metadata.clone());
                me.plugin_event_sender = Some(manager.get_event_sender());
                plugin_manager = Some(Arc::new(Mutex::new(manager)));

                // Sync UI items (preserved logic)
                // Sync UI items (restored logic)
                if let Some(ref pm) = plugin_manager {
                    let pm_lock = pm.lock();

                    // Sync top-level tabs to Toolbar
                    let top_tabs = pm_lock.get_all_top_tabs();
                    let mut ui_items = Vec::new();

                    for (plugin_id, tab) in top_tabs {
                        let item = UiItem {
                            id: format!("plugin:{}:{}", plugin_id, tab.id),
                            region: UiRegion::Toolbar,
                            group_id: Some("plugins".to_string()),
                            label: tab.label,
                            icon: Some(tab.icon),
                            visible: true,
                            sort_order: tab.priority as i32, // Plugins manage their own priority relative to each other
                            display_mode: DisplayMode::IconAndText,
                            action_type: ActionType::Plugin,
                            // action_data format: "plugin_id:page_id"
                            action_data: Some(format!("{}:{}", plugin_id, tab.id)),
                        };
                        ui_items.push(item);
                    }

                    // Upsert items if we have a UI service
                    if let Some(ui_svc) = services.ui_service.clone() {
                        if !ui_items.is_empty() {
                            if let Err(e) = ui_svc.upsert_items(&ui_items) {
                                warn!("Failed to sync plugin UI items: {}", e);
                            } else {
                                info!("Synced {} plugin UI items to database", ui_items.len());
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }

        // Finalize services
        // services.plugin_manager = plugin_manager.clone(); // Removed from Services struct

        // Apply UI config
        let services = crate::core::services::Services {
            core: services,
            plugin_manager,
            content_cache,
            resource_manager,
        };

        // Load UI items via UiService now that services are ready
        // Access via Deref
        if let Some(ref svc) = services.ui_service {
            if let Ok(items) = svc.list_toolbar_items() {
                me.signals.toolbar_items.set(items);
            }
            if let Ok(items) = svc.list_info_panel_items() {
                me.signals.info_panel_items.set(items);
            }
        }

        Ok((me, services))
    }

    pub fn list_archive(&mut self, path: &Path) -> Result<Vec<arclain_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        self.signals.current_password.set(None);

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
                                self.signals.current_password.set(Some(password.clone()));
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
                    self.signals.current_password.set(Some(password.clone()));
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        // IMPORTANT: Set archive_path signal BEFORE password detection so archive name matching works
        let archive_path = Some(path.to_path_buf());
        self.signals.archive_path.set(archive_path.clone());

        // Update archive_info signal directly (no AppState copy)
        {
            let mut ai = self.signals.archive_info.get();
            ai.archive_encrypted = info.encrypted;
            ai.headers_encrypted = info.headers_encrypted;
            ai.encryption_method = info.encryption_method.clone();
            self.signals.archive_info.set(ai);
        }
        crate::core::operations::navigation_signals::reset_navigation(&self.signals);

        // Update reactive signals for async UI updates
        self.signals
            .entries
            .set(std::sync::Arc::new(info.entries.clone()));

        // Now attempt password detection with correct archive context
        if self.signals.current_password.get().is_none() {
            let archive_name = archive_path.as_ref().and_then(|p| p.to_str());
            debug!(
                "Attempting auto-password detection for archive: {:?}",
                archive_name
            );
            let detected_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
            if let Some(ref pwd) = detected_pw {
                info!("Auto-detected password for archive (length: {})", pwd.len());
                self.signals.current_password.set(Some(pwd.clone()));
            } else if info.encrypted {
                warn!("Archive is encrypted but no password was auto-detected from rules");
            } else {
                debug!("No password needed - archive is not encrypted");
            }
        } else {
            info!(
                "Password already set (length: {})",
                self.signals
                    .current_password
                    .get()
                    .as_ref()
                    .map(|p| p.len())
                    .unwrap_or(0)
            );
        }

        // Store OnArchiveOpen event for deferred dispatch (after UI renders)
        // This prevents plugins from fetching metadata before the UI is ready
        self.signals.metadata.set(None); // Reset metadata via signal
        if self.plugin_event_sender.is_some() {
            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().to_string(),
                kind: info.archive_kind,
                password: self.signals.current_password.get(),
            };

            // Store event for deferred dispatch - will be sent after UI renders
            self.pending_plugin_event = Some(event);
            // Signal that UI needs to render before plugin events fire
            self.signals.ui_ready.set(false);

            info!(
                "Archive opened successfully with {} entries (plugin event pending)",
                self.signals.entries.get().len()
            );
        }

        if self.plugin_event_sender.is_none() {
            info!(
                "Archive opened successfully with {} entries",
                self.signals.entries.get().len()
            );
        }
        Ok(self.signals.entries.get().as_ref().clone())
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
        let archive_path = Some(path.to_path_buf());
        self.signals.archive_path.set(archive_path);

        // Update archive_info signal directly (no AppState copy)
        {
            let mut ai = self.signals.archive_info.get();
            ai.archive_encrypted = info.encrypted;
            ai.headers_encrypted = info.headers_encrypted;
            ai.encryption_method = info.encryption_method.clone();
            self.signals.archive_info.set(ai);
        }
        crate::core::operations::navigation_signals::reset_navigation(&self.signals);
        self.signals
            .current_password
            .set(Some(password.to_string()));

        // Store OnArchiveOpen event for deferred dispatch (after UI renders)
        // This prevents plugins from fetching metadata before the UI is ready
        if self.plugin_event_sender.is_some() {
            use arclain_plugins::PluginEvent;
            let event = PluginEvent::OnArchiveOpen {
                path: path.to_string_lossy().to_string(),
                kind: info.archive_kind.clone(),
                password: Some(password.to_string()),
            };

            // Store event for deferred dispatch - will be sent after UI renders
            self.pending_plugin_event = Some(event);
            // Signal that UI needs to render before plugin events fire
            self.signals.ui_ready.set(false);
        }

        Ok(self.signals.entries.get().as_ref().clone())
    }

    /// Dispatch any pending plugin event after UI has rendered.
    /// This should be called once after the file list is first rendered
    /// following an archive open operation.
    pub fn dispatch_pending_plugin_event(&mut self) {
        if let Some(event) = self.pending_plugin_event.take() {
            debug!("Dispatching deferred plugin event after UI render");

            // Send to plugin worker via channel
            if let Some(ref sender) = self.plugin_event_sender {
                if let Err(e) = sender.send(event) {
                    warn!("Failed to send deferred event to plugin worker: {}", e);
                }
            }

            // Mark UI as ready
            self.signals.ui_ready.set(true);
        }
    }

    pub fn get_current_entries(&self) -> Vec<arclain_core::ArchiveEntry> {
        self.signals
            .navigation
            .get()
            .filter_entries(&self.signals.entries.get())
    }

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        let backend = self.backend_selector.select(archive)?;
        backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let archive_name = archive.to_str();
        let auto_pw = auto_password_for(&self.pass_rules, archive_name, &self.last_entries);
        let signal_pw = self.signals.current_password.get();
        let pw = signal_pw.as_deref().or(auto_pw.as_deref());
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
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Start from existing paths or defaults
        let mut paths = if let Some(p) = self.db_paths.clone() {
            p
        } else {
            DbPaths::calculate_defaults("arclain")?
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
            if let Some(manager_arc) = plugin_manager {
                let lib_svc = arclain_core::LibraryService::new(dbs.cache_pool.clone());
                manager_arc.lock().set_library_service(Arc::new(lib_svc));
            }

            self.dbs = Some(dbs);

            // Reload pass rules
            if let Some(ref dbs_ref) = self.dbs {
                if let Ok(rules) = dbs_ref.secrets.list_pass_rules() {
                    self.pass_rules = rules
                        .into_iter()
                        .map(|r| PassRule {
                            name: r.name,
                            pattern: r.pattern,
                            password: r.password,
                            priority: r.priority,
                            enabled: r.enabled,
                        })
                        .collect();
                }
            }
        } else {
            // No key provided, still keep updated paths
            self.db_paths = Some(paths.clone());
        }

        Ok(())
    }

    pub fn move_vault(
        &mut self,
        dest_path: &str,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Close existing DBs to avoid file locks
        if let Some(_) = self.dbs.take() {
            // dropped
        }

        let (new_paths, new_dbs) = SecretsService::move_vault(&mut self.db_paths, dest_path)?;

        // Update state with new paths and DBs
        self.db_paths = Some(new_paths.clone());

        // Update plugin manager
        if let Some(manager_arc) = plugin_manager {
            let lib_svc = arclain_core::LibraryService::new(new_dbs.cache_pool.clone());
            manager_arc.lock().set_library_service(Arc::new(lib_svc));
        }

        self.dbs = Some(new_dbs);

        // Reload rules
        if let Some(ref dbs_ref) = self.dbs {
            if let Ok(rules) = dbs_ref.secrets.list_pass_rules() {
                self.pass_rules = rules
                    .into_iter()
                    .map(|r| PassRule {
                        name: r.name,
                        pattern: r.pattern,
                        password: r.password,
                        priority: r.priority,
                        enabled: r.enabled,
                    })
                    .collect();
            }
        }

        Ok(())
    }

    pub fn rekey_vault(
        &mut self,
        new_key_file_path: &str,
        plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    ) -> Result<()> {
        // Close existing DBs
        if let Some(_) = self.dbs.take() {
            // dropped
        }

        let (new_paths, new_dbs, rules) =
            SecretsService::rekey_vault(&mut self.db_paths, new_key_file_path)?;

        self.db_paths = Some(new_paths.clone());

        // Update plugin manager
        if let Some(manager_arc) = plugin_manager {
            let lib_svc = arclain_core::LibraryService::new(new_dbs.cache_pool.clone());
            manager_arc.lock().set_library_service(Arc::new(lib_svc));
        }

        self.dbs = Some(new_dbs);
        self.pass_rules = rules
            .into_iter()
            .map(|r| PassRule {
                name: r.name,
                pattern: r.pattern,
                password: r.password,
                priority: r.priority,
                enabled: r.enabled,
            })
            .collect();

        Ok(())
    }

    /// Save password rules to the encrypted secrets database
    pub fn save_password_rules(&mut self, rules: Vec<PassRule>) -> Result<()> {
        // Update in-memory cache
        self.pass_rules = rules.clone();

        // Persist to secrets DB if available
        if let Some(ref dbs) = self.dbs {
            let db_rules: Vec<DbPassRule> = rules
                .into_iter()
                .map(|r| DbPassRule {
                    name: r.name,
                    pattern: r.pattern,
                    password: r.password,
                    priority: r.priority,
                    enabled: r.enabled,
                })
                .collect();
            if let Err(e) = dbs.secrets.replace_all_pass_rules(&db_rules) {
                warn!("Failed to save password rules to DB: {}", e);
            } else {
                info!(
                    "Saved {} password rules to encrypted secrets DB",
                    db_rules.len()
                );
            }
        } else {
            // No DB available - can't save
            warn!("Cannot save password rules - DB not available (rules updated in memory only)");
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
            let config_pool = &dbs.config_pool;
            // Run sync in background or just block? Startup is fine to block briefly.
            if let Err(e) = arclain_core::config::sync::sync_rules(config_pool) {
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

    /// Refresh UI configuration (toolbar/info panel items) from UiService
    pub fn reload_ui_config(&mut self, ui_service: &arclain_core::UiService) {
        if let Ok(items) = ui_service.list_toolbar_items() {
            self.signals.toolbar_items.set(items);
        }
        if let Ok(items) = ui_service.list_info_panel_items() {
            self.signals.info_panel_items.set(items);
        }
    }
}
