//! Central manager for all core services
//!
//! Handles initialization, connection pooling, and dependency injection.

use crate::services::{
    CacheService, ConfigService, LibraryService, OrganizationService, UiService,
};
use anyhow::Result;
use arclain_db::DbPaths;
use arclain_http::features::whitelist::DomainWhitelist;
use arclain_http::AsyncHttpClient;
// PluginManager removed to avoid circular dependency
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Container for all core domain services
#[derive(Clone)]
pub struct Services {
    pub tokio_runtime: Arc<tokio::runtime::Runtime>,
    pub async_http_client: Arc<AsyncHttpClient>,
    pub domain_whitelist: Arc<RwLock<DomainWhitelist>>,

    // Database Paths and Connection
    pub db_paths: Option<DbPaths>,

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
            library_service: None,
            organization_service: None,
            config_service: None,
            ui_service: None,
            cache_service: None,
            cache_dir: PathBuf::new(),
            // plugin_manager: None,
            // plugin_event_sender: None,
            checksum_service: None,
        }
    }

    /// Initialize database-dependent services
    pub fn init_db_services(&mut self, dbs: &arclain_db::ConfigDbs, paths: &DbPaths) -> Result<()> {
        self.db_paths = Some(paths.clone());

        // Create core services
        let config_svc = Arc::new(ConfigService::from_connection(
            dbs.config_pool.clone(),
            arclain_db::DbConnection::open(&paths.config_db)?,
        ));

        // Cache Service
        let cache_svc = Arc::new(CacheService::new(dbs.cache_pool.clone()));

        // Library Service
        let library_svc = Arc::new(LibraryService::new(dbs.cache_pool.clone()));

        // Organization Service
        let org_svc = Arc::new(OrganizationService::new(dbs.config_pool.clone()));

        // UI Service
        let ui_svc = Arc::new(UiService::new(dbs.config_pool.clone()));

        // --- Cache Path Fix ---
        // Separate cache folder from DB folder.
        // If config_db is %APPDATA%/arclain/config.sqlite, parent is %APPDATA%/arclain.
        // We want %APPDATA%/arclain/cache.
        // Cache Path Fix:
        // config_db is .../databases/config.sqlite
        // parent is .../databases
        // grandparent is .../ (app root)
        // we want .../cache
        let cache_dir = paths
            .config_db
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(Path::new("."))
            .join("cache");

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
