//! Central manager for all core services
//!
//! Handles initialization, connection pooling, and dependency injection.

use crate::services::{
    CacheService, ConfigService, LibraryService, OrganizationService, UiService,
};
use anyhow::Result;
use arclain_db::{DbPaths, SqliteDb};
use arclain_network::features::gameta_client::{GametaClient, ServerConfig};
use arclain_network::features::whitelist::DomainWhitelist;
use arclain_network::AsyncHttpClient;
// PluginManager removed to avoid circular dependency
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

/// Container for all core domain services
#[derive(Clone)]
pub struct Services {
    pub tokio_runtime: Arc<tokio::runtime::Runtime>,
    pub async_http_client: Arc<AsyncHttpClient>,
    pub domain_whitelist: Arc<RwLock<DomainWhitelist>>,

    // Database Paths and Connection
    pub db_paths: Option<DbPaths>,
    /// Shared handle to the config SQLite database, used by the pipeline
    /// executor for idempotent-run dedup and crash recovery via
    /// `arclain_db::pipeline_runs`.
    pub config_db: Option<Arc<SqliteDb>>,

    // Core Domain Services
    pub library_service: Option<Arc<LibraryService>>,
    pub organization_service: Option<Arc<OrganizationService>>,
    pub config_service: Option<Arc<ConfigService>>,
    pub ui_service: Option<Arc<UiService>>,
    pub cache_service: Option<Arc<CacheService>>,

    // Path Management
    pub cache_dir: PathBuf,

    // External Integration
    // plugin_manager removed to avoid cycle
    // pub plugin_event_sender: Option<std::sync::mpsc::Sender<arclain_plugins::PluginEvent>>,
    pub checksum_service: Option<Arc<crate::utilities::ChecksumService>>,

    // Gameta server client
    pub gameta_client: Option<Arc<GametaClient>>,
}

impl Services {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        let domain_whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));

        // Initialize AsyncHttpClient
        let async_http_client = Arc::new(AsyncHttpClient::new(
            runtime.handle().clone(),
            domain_whitelist.clone(),
            None,
        ));

        Self {
            tokio_runtime: runtime,
            async_http_client,
            domain_whitelist,
            db_paths: None,
            config_db: None,
            library_service: None,
            organization_service: None,
            config_service: None,
            ui_service: None,
            cache_service: None,
            cache_dir: PathBuf::new(),
            // plugin_manager: None,
            // plugin_event_sender: None,
            checksum_service: None,
            gameta_client: None,
        }
    }

    /// Initialize database-dependent services
    pub fn init_db_services(&mut self, dbs: &arclain_db::ConfigDbs, paths: &DbPaths) -> Result<()> {
        self.db_paths = Some(paths.clone());
        self.config_db = Some(Arc::new(dbs.config.clone()));

        // Crash recovery: any `in_progress` pipeline_runs row older than an
        // hour can only exist because a previous arclain process died mid-run.
        // Flip such rows to `failed` with `error = "interrupted"` so the UI
        // can surface them and nothing downstream thinks the work is alive.
        const STALE_THRESHOLD_SECS: i64 = 3600;
        if let Err(e) = dbs.config.with_connection(|conn| {
            Ok(arclain_db::flag_stale_in_progress(conn, STALE_THRESHOLD_SECS)?)
        }) {
            tracing::warn!("[services] Failed to sweep stale pipeline runs: {}", e);
        }

        // Create core services
        let config_svc = Arc::new(ConfigService::from_connection(
            dbs.config_pool.clone(),
            arclain_db::DbConnection::open(&paths.config_db)?,
        ));

        // Cache Service
        let cache_svc = Arc::new(CacheService::new(dbs.cache_pool.clone()));

        // Library Service (uses gameta_database::DieselBackend for metadata CRUD)
        let library_svc = Arc::new(LibraryService::new(&paths.cache_db)?);

        // Organization Service
        let org_svc = Arc::new(OrganizationService::new(dbs.config_pool.clone()));

        // UI Service
        let ui_svc = Arc::new(UiService::new(dbs.config_pool.clone()));

        // Use centralized directory logic
        let app_dirs = arclain_app_fs::AppDirectories::init("arclain", None)?;
        let cache_dir = app_dirs.cache_dir;

        // --- Proxy Configuration ---
        if let Ok(mut conn) = dbs.config_pool.get() {
            if let Ok(user_config) = arclain_db::UserConfig::load_diesel(&mut conn) {
                let proxy_config =
                    crate::utilities::proxy::resolve_proxy_config(&user_config, &dbs.secrets);
                crate::utilities::proxy::apply_proxy_to_client(
                    &self.async_http_client,
                    proxy_config,
                    &user_config,
                );

                // --- Gameta Server Client ---
                if user_config.gameta_server_enabled {
                    if let Some(url) = user_config.gameta_server_url.clone() {
                        let api_key = match dbs.secrets.get_secret("gameta:api_key") {
                            Ok(Some(key)) => {
                                let key_str: &str = key.as_ref();
                                Some(key_str.to_string())
                            }
                            Ok(None) => None,
                            Err(e) => {
                                tracing::warn!(
                                    "[GametaClient] Failed to load API key from secrets: {}",
                                    e
                                );
                                None
                            }
                        };

                        let config = ServerConfig { url, api_key };
                        let client = GametaClient::new(config);

                        match client.health() {
                            Ok(resp) => {
                                tracing::info!(
                                    "[GametaClient] Connected to gameta server \
                                     (status: {}, version: {})",
                                    resp.status,
                                    resp.version
                                );
                                self.gameta_client = Some(Arc::new(client));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[GametaClient] Health check failed, \
                                     gameta integration disabled: {}",
                                    e
                                );
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[GametaClient] gameta_server_enabled is true \
                             but no URL is configured"
                        );
                    }
                }
            }
        }

        // Directory creation removed - caller responsibility

        // Assign to self
        self.config_service = Some(config_svc);
        self.cache_service = Some(cache_svc);
        self.library_service = Some(library_svc);
        self.organization_service = Some(org_svc);
        self.ui_service = Some(ui_svc);
        self.cache_dir = cache_dir;

        Ok(())
    }
}
