//! Central manager for all core services
//!
//! Handles initialization, connection pooling, and dependency injection.

#[cfg(feature = "gameta")]
use crate::services::LibraryService;
use crate::services::{
    CacheService, ConfigService, NetworkProxyPersistenceService, OrganizationService,
    ProxyRecoveryOutcome, UiService,
};
use anyhow::{Context, Result};
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
    #[cfg(feature = "gameta")]
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
            #[cfg(feature = "gameta")]
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
            Ok(arclain_db::flag_stale_in_progress(
                conn,
                STALE_THRESHOLD_SECS,
            )?)
        }) {
            tracing::warn!("[services] Failed to sweep stale pipeline runs: {}", e);
        }

        // Create core services
        let config_svc = Arc::new(ConfigService::from_connection(
            dbs.config_pool.clone(),
            arclain_db::DbConnection::open(&paths.config_db)?,
        ));
        let recovery =
            NetworkProxyPersistenceService::new(&config_svc, &dbs.secrets).recover_pending();
        let recovery = match recovery {
            Ok(outcome) => outcome,
            Err(error) => {
                self.async_http_client.mark_plugin_routing_unavailable();
                return Err(error).context("recovering pending proxy settings update");
            }
        };
        match recovery {
            ProxyRecoveryOutcome::NoPendingUpdate => {}
            ProxyRecoveryOutcome::RolledBack => {
                tracing::warn!("[services] Rolled back an interrupted proxy settings update");
            }
            ProxyRecoveryOutcome::Finalized => {
                tracing::info!("[services] Finalized an interrupted proxy settings update");
            }
        }

        // Cache Service
        let cache_svc = Arc::new(CacheService::new(dbs.cache_pool.clone()));

        // Library Service (uses gameta_database::DieselBackend for metadata CRUD)
        #[cfg(feature = "gameta")]
        let library_svc = Arc::new(LibraryService::new(&paths.cache_db)?);

        // Organization Service
        let org_svc = Arc::new(OrganizationService::new(dbs.config_pool.clone()));

        // UI Service
        let ui_svc = Arc::new(UiService::new(dbs.config_pool.clone()));

        // Use centralized directory logic
        let app_dirs = arclain_app_fs::AppDirectories::init("arclain", None)?;
        let cache_dir = app_dirs.cache_dir;

        // --- Proxy Configuration ---
        match dbs.config_pool.get() {
            Ok(mut conn) => match arclain_db::UserConfig::load_diesel(&mut conn) {
                Ok(user_config) => {
                    let proxy_config = match crate::utilities::proxy::resolve_proxy_config(
                        &user_config,
                        &dbs.secrets,
                    ) {
                        Ok(config) => config,
                        Err(error) => {
                            self.async_http_client.mark_plugin_routing_unavailable();
                            return Err(error).context("resolving persisted proxy configuration");
                        }
                    };
                    crate::utilities::proxy::apply_proxy_to_client(
                        &self.async_http_client,
                        proxy_config,
                        &user_config,
                    )
                    .context("applying persisted proxy routing")?;

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
                Err(error) => {
                    self.async_http_client.mark_plugin_routing_unavailable();
                    return Err(error).context("loading user config for proxy routing");
                }
            },
            Err(error) => {
                self.async_http_client.mark_plugin_routing_unavailable();
                return Err(error).context("acquiring connection to load user config");
            }
        }

        // Directory creation removed - caller responsibility

        // Assign to self
        self.config_service = Some(config_svc);
        self.cache_service = Some(cache_svc);
        #[cfg(feature = "gameta")]
        {
            self.library_service = Some(library_svc);
        }
        self.organization_service = Some(org_svc);
        self.ui_service = Some(ui_svc);
        self.cache_dir = cache_dir;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ConfigService;
    use arclain_db::{open_databases, DbConnection, SecretsKey, UserConfig};
    use arclain_network::{HttpRequest, PluginNetworkPolicy};

    const CHECKED_PLUGIN_ID: &str = "dlsite-metadata";

    fn configure_checked_plugin(services: &Services) {
        services.async_http_client.configure_plugin(
            CHECKED_PLUGIN_ID,
            PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 60,
            },
        );
    }

    fn assert_checked_plugin_routing_unavailable(services: &Services) {
        let error = services
            .async_http_client
            .request_for_plugin(
                CHECKED_PLUGIN_ID,
                HttpRequest::get("https://example.com/resource"),
            )
            .expect_err("checked plugin request must fail while routing is unavailable");
        assert!(
            error.to_string().contains("routing is unavailable"),
            "unexpected unavailable-routing error: {error}"
        );
    }

    #[test]
    fn init_db_services_rejects_corrupt_proxy_marker_before_proxy_application() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let dbs = open_databases(&paths, &SecretsKey::generate()).unwrap();
        let connection = DbConnection::open(&paths.config_db).unwrap();
        UserConfig::ensure_table(&connection).unwrap();
        let config_service = ConfigService::from_connection(dbs.config_pool.clone(), connection);
        let mut config = UserConfig::new();
        config.socks5_enabled = true;
        config.socks5_address = Some("127.0.0.1:1080".to_string());
        config_service.save_user_config(&config).unwrap();
        dbs.secrets
            .set_secret("proxy:socks5", "proxy-password")
            .unwrap();
        dbs.secrets
            .set_secret("journal:proxy-settings", "invalid-marker")
            .unwrap();

        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let mut services = Services::new(runtime);
        configure_checked_plugin(&services);
        services.async_http_client.apply_proxy_routing(
            crate::utilities::proxy::resolve_proxy_config(&config, &dbs.secrets).unwrap(),
            crate::utilities::effective_plugin_proxy_map(&config),
        );

        let error = services.init_db_services(&dbs, &paths).unwrap_err();

        assert!(
            format!("{error:#}").contains("pending proxy update marker"),
            "{error:#}"
        );
        assert_checked_plugin_routing_unavailable(&services);
    }

    #[test]
    fn init_db_services_fails_closed_when_credential_secret_cannot_be_decrypted() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let original_key = SecretsKey::generate();
        {
            let dbs = open_databases(&paths, &original_key).unwrap();
            let connection = DbConnection::open(&paths.config_db).unwrap();
            UserConfig::ensure_table(&connection).unwrap();
            let config_service =
                ConfigService::from_connection(dbs.config_pool.clone(), connection);
            let mut config = UserConfig::new();
            config.socks5_enabled = true;
            config.socks5_address = Some("127.0.0.1:1080".to_string());
            config.socks5_username = Some("proxy-user".to_string());
            config_service.save_user_config(&config).unwrap();
            dbs.secrets
                .set_secret("proxy:socks5", "proxy-password")
                .unwrap();
        }

        let dbs = open_databases(&paths, &SecretsKey::generate()).unwrap();
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let mut services = Services::new(runtime);
        configure_checked_plugin(&services);

        let error = services.init_db_services(&dbs, &paths).unwrap_err();

        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("proxy password"), "{diagnostic}");
        assert!(!diagnostic.contains("proxy-password"), "{diagnostic}");
        assert!(!diagnostic.contains("proxy-user"), "{diagnostic}");
        assert_checked_plugin_routing_unavailable(&services);
    }

    #[test]
    fn init_db_services_does_not_read_proxy_secret_for_credentialless_proxy() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let original_key = SecretsKey::generate();
        {
            let dbs = open_databases(&paths, &original_key).unwrap();
            let connection = DbConnection::open(&paths.config_db).unwrap();
            UserConfig::ensure_table(&connection).unwrap();
            let config_service =
                ConfigService::from_connection(dbs.config_pool.clone(), connection);
            let mut config = UserConfig::new();
            config.socks5_enabled = true;
            config.socks5_address = Some("127.0.0.1:1080".to_string());
            config_service.save_user_config(&config).unwrap();
            dbs.secrets
                .set_secret("proxy:socks5", "unreadable-but-unused-password")
                .unwrap();
        }

        let dbs = open_databases(&paths, &SecretsKey::generate()).unwrap();
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let mut services = Services::new(runtime);
        configure_checked_plugin(&services);

        services.init_db_services(&dbs, &paths).unwrap();

        assert!(
            services
                .async_http_client
                .should_use_proxy_for_plugin(CHECKED_PLUGIN_ID),
            "credentialless proxy did not install its default plugin route"
        );
    }

    #[test]
    fn init_db_services_fails_closed_when_user_config_cannot_be_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let dbs = open_databases(&paths, &SecretsKey::generate()).unwrap();
        dbs.config
            .with_connection(|connection| {
                connection.execute_batch("DROP TABLE user_config")?;
                Ok(())
            })
            .unwrap();

        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let mut services = Services::new(runtime);
        configure_checked_plugin(&services);

        let error = services.init_db_services(&dbs, &paths).unwrap_err();

        assert!(error.to_string().contains("user config"), "{error:#}");
        assert_checked_plugin_routing_unavailable(&services);
    }
}
