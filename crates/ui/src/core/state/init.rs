//! AppState initialization

use super::AppState;
use crate::core::signals::AppSignals;
use anyhow::Result;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::services::Services as CoreServices;
use arclain_core::services::{ConfigService, SecretsService};
use arclain_core::utilities::{ChecksumService, PassRule};
use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion, UserConfig};
use arclain_data::{ContentCache, ResourceConfig, ResourceManager};
use arclain_db::{open_databases, DbPaths, SecretsKey};
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, info, warn};

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

        // Initialize plugin system
        info!("Initializing plugin system");
        let plugins_dir = PathBuf::from("plugins");
        let backend_arc = Arc::new(me.fallback_backend.clone());
        let settings = me.user_config.get_all_plugin_settings();
        match PluginManager::with_backend(plugins_dir, backend_arc, settings) {
            Ok(mut manager) => {
                manager.init().ok();
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

                // Sync UI items
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
                            sort_order: tab.priority as i32,
                            display_mode: DisplayMode::IconAndText,
                            action_type: ActionType::Plugin,
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
        let services = crate::core::services::Services {
            core: services,
            plugin_manager,
            content_cache,
            resource_manager,
        };

        // Load UI items via UiService now that services are ready
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
}
