//! The actual `ArclainApp::bootstrap` composition sequence.
//!
//! Moved, preserving order, from `crates/ui/src/core/state/init.rs`'s
//! `AppState::new`: directories, plugin directory resolution,
//! configuration, user configuration, databases, backend selector and
//! fallback, content cache, resource manager, checksum service, plugin
//! manager, plugin service injection, plugin scheduler, and persisted
//! state (syncing plugin-provided UI items into the database).
//!
//! Two steps named in that same characterization deliberately have *no*
//! code here:
//!
//! - **Logging.** Installing the global `tracing` subscriber
//!   (`arclain_core::utilities::init_logging`) stays a one-time call in
//!   the UI binary's `main()`. A subscriber can only be installed once
//!   per process; calling it from here would break repeated
//!   bootstrap/drop (see `crates/app/tests/bootstrap.rs`), which the
//!   runtime contract explicitly requires to keep working. `log_dir`'s
//!   *path* is still resolved as part of `AppPaths`.
//! - **Active context.** The live bridge from the plugin system to "what
//!   tab is currently active" (`arclain_plugins::ActiveTabBridge`) is
//!   this crate's own [`crate::plugins::ArchiveContextBridge`], obtained
//!   via [`crate::ArclainApp::active_tab_bridge`] -- but installing it
//!   still cannot happen here: it needs a caller-supplied fallback
//!   closure for the one case archive-session state alone cannot
//!   resolve, and `crates/ui`'s own closure writes into `AppSignals`, an
//!   egui-integration type this crate must never depend on. `crates/ui`
//!   calls `active_tab_bridge` and wires the result onto the
//!   `PluginManager` it gets back through
//!   [`crate::ArclainApp::take_legacy_composition`] immediately after
//!   this function returns, in the same relative position this step
//!   held in the original sequence.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex as SyncMutex;
use tracing::{info, warn};

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::{BackendSelector, UnrarCli};
use arclain_core::services::{ConfigService, Services as CoreServices};
use arclain_core::utilities::{effective_plugin_proxy_map, ChecksumService, PassRule};
use arclain_core::{open_databases, ConfigDbs, DbPaths, SecretsKey};
use arclain_core::{ActionType, DisplayMode, UiItem, UiRegion, UserConfig};
use arclain_core::{ContentCache, ResourceConfig, ResourceManager};
use arclain_plugins::PluginManager;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::operations::{ChallengeWaiters, OperationRegistry};

use super::paths::AppPaths;
use super::session_store::SessionStore;
use super::AppRuntime;

/// What [`crate::ArclainApp::bootstrap`] needs beyond the OS/
/// installation defaults.
#[derive(Clone)]
pub struct BootstrapConfig {
    /// Overrides every OS-conventional directory `AppPaths::system_default`
    /// would otherwise compute. Tests use this to point an entire
    /// profile at a temp directory; a real deployment leaves it `None`.
    pub paths_override: Option<AppPaths>,
    /// Worker thread count for the application-owned Tokio runtime.
    /// `None` uses Tokio's own default (the number of logical CPUs).
    pub worker_threads: Option<usize>,
    /// Test-only seam: when set, every `start_open_archive` call uses this
    /// backend instead of selecting one by file extension. Lets a test
    /// exercise the real `start_open_archive` operation (challenges,
    /// retries, cancellation) against a deterministic fake backend without
    /// depending on a real encrypted archive fixture. Always `None` in
    /// `system_default()`.
    ///
    /// Reused by the Task 9 processing operations (`start_convert`/
    /// `start_organize`/`start_pipeline`) for exactly the same reason:
    /// `arclain_core::PipelineContext::backend_for` needs to resolve an
    /// extraction backend for each input, and this is the one seam that
    /// already exists for "which backend resolves a given path" -- see
    /// `runtime::processing_ops::build_pipeline_context`.
    pub archive_backend_override: Option<Arc<dyn arclain_core::ArchiveBackend>>,
    /// Test-only seam: when set, every `start_extract` call spawns
    /// through this runner instead of the real 7-Zip CLI. Lets a test
    /// exercise the real `start_extract` operation (progress, collision/
    /// password challenges, retries, cancellation) deterministically --
    /// see `crate::operations::extract::ExtractRunner`. Always `None` in
    /// `system_default()`.
    pub extract_runner_override: Option<Arc<dyn crate::operations::extract::ExtractRunner>>,
    /// Test-only seam: overrides how long a materialization lease stays
    /// valid before expiring. `None` uses
    /// `crate::materialization::DEFAULT_LEASE_TTL`. A test that needs to
    /// observe real expiry cleanup sets this (and usually
    /// `materialization_cleanup_interval_override` too) to a much shorter
    /// duration rather than waiting on the production TTL. Always `None`
    /// in `system_default()`.
    pub materialization_lease_ttl_override: Option<std::time::Duration>,
    /// Test-only seam: overrides how often the background sweep that
    /// removes expired materialization leases runs. `None` uses
    /// `crate::materialization::DEFAULT_CLEANUP_INTERVAL`. Always `None`
    /// in `system_default()`.
    pub materialization_cleanup_interval_override: Option<std::time::Duration>,
    /// An already-prepared plugin-routing generation owned by the embedding
    /// host. `None` preserves standalone persisted routing behavior.
    pub initial_plugin_network_routing: Option<arclain_network::PreparedPluginNetworkRouting>,
}

/// Application-owned fixture overrides that should not require a
/// frontend to edit backend configuration storage before bootstrap.
#[derive(Clone, Debug, Default)]
pub struct BootstrapOverrides {
    /// Explicit 7-Zip executable used for this process only. It takes
    /// precedence over the persisted setting and is never written back.
    pub sevenzip_path: Option<PathBuf>,
}

/// Hand-written rather than `#[derive(Debug)]`: `dyn arclain_core::
/// ArchiveBackend` does not implement `Debug`, so `archive_backend_override`
/// cannot derive it. Reports only whether an override is set, never the
/// backend's identity or contents.
impl std::fmt::Debug for BootstrapConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapConfig")
            .field("paths_override", &self.paths_override)
            .field("worker_threads", &self.worker_threads)
            .field(
                "archive_backend_override_is_set",
                &self.archive_backend_override.is_some(),
            )
            .field(
                "extract_runner_override_is_set",
                &self.extract_runner_override.is_some(),
            )
            .field(
                "materialization_lease_ttl_override",
                &self.materialization_lease_ttl_override,
            )
            .field(
                "materialization_cleanup_interval_override",
                &self.materialization_cleanup_interval_override,
            )
            .field(
                "initial_plugin_network_routing_is_set",
                &self.initial_plugin_network_routing.is_some(),
            )
            .finish()
    }
}

impl BootstrapConfig {
    /// The configuration a real, non-test launch uses: OS-conventional
    /// paths, default worker thread count, no backend/runner overrides.
    pub fn system_default() -> Self {
        Self {
            paths_override: None,
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
            initial_plugin_network_routing: None,
        }
    }
}

fn internal_error(context: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "failed to start the application",
    )
    .with_diagnostic(format!("{context}: {error}"))
    .with_recoverability(Recoverability::Fatal)
}

/// Builds the content cache and resource manager from one
/// [`ResourceConfig`] so their disk limits cannot silently diverge.
fn initialize_resource_services(
    cache_dir: PathBuf,
    cache_index: Arc<dyn arclain_core::CacheIndex>,
    resource_config: ResourceConfig,
) -> anyhow::Result<(Arc<ContentCache>, Arc<ResourceManager>)> {
    let cache = Arc::new(ContentCache::new_with_config(
        cache_dir,
        cache_index,
        &resource_config,
    )?);
    let manager = Arc::new(ResourceManager::new(cache.clone(), resource_config));
    Ok((cache, manager))
}

/// Broadens stored password rules that still carry the one-archive-only
/// pattern auto-saving produced before the pattern heuristic existed,
/// writing the result back and updating `pass_rules` in place. Returns
/// how many rules changed.
///
/// Runs here, during composition, rather than in a frontend: it rewrites
/// persisted secrets, and it has to land before *anything* can read a
/// rule -- an archive opened against the un-upgraded set would re-prompt
/// for a password the vault already holds. Idempotent, so running it on
/// every launch costs one list-and-compare after the first: a broadened
/// pattern no longer matches the narrow fingerprint
/// `upgrade_auto_saved_rules` requires, so a second pass reports nothing
/// to do.
///
/// Best-effort on the write: a vault that refuses the update leaves the
/// narrow rules in place (they still work, they just match less than
/// they could) rather than failing the whole bootstrap, and this
/// function reports 0 so no frontend claims an upgrade that did not
/// happen.
fn upgrade_narrow_auto_saved_rules(dbs: &ConfigDbs, pass_rules: &mut Vec<PassRule>) -> usize {
    let Some(upgraded) =
        arclain_core::utilities::password_matcher::upgrade_auto_saved_rules(pass_rules)
    else {
        return 0;
    };
    let changed = upgraded
        .iter()
        .zip(pass_rules.iter())
        .filter(|(new, old)| new.pattern != old.pattern)
        .count();

    let db_rules: Vec<arclain_core::DbPassRule> = upgraded
        .iter()
        .cloned()
        .map(|rule| arclain_core::DbPassRule {
            name: rule.name,
            pattern: rule.pattern,
            password: rule.password,
            priority: rule.priority,
            enabled: rule.enabled,
        })
        .collect();
    if let Err(error) = dbs.secrets.replace_all_pass_rules(&db_rules) {
        warn!("Failed to persist upgraded password rules: {error}");
        return 0;
    }

    info!("Broadened {changed} auto-saved password rule(s) to match sibling archives");
    *pass_rules = upgraded;
    changed
}

pub(crate) fn run(
    config: BootstrapConfig,
    overrides: BootstrapOverrides,
) -> Result<AppRuntime, ApplicationError> {
    info!("Bootstrapping application runtime");

    // -- directories, plugin directory resolution --
    let paths = AppPaths::resolve(config.paths_override)?;
    paths.ensure_created()?;

    // Application-owned Tokio runtime. Every internal spawn/
    // spawn_blocking/timer this crate performs from here on goes
    // through this handle, never an ambient one -- see this module's
    // doc comment and `crate::runtime`'s own.
    let mut runtime_builder = tokio::runtime::Builder::new_multi_thread();
    if let Some(worker_threads) = config.worker_threads {
        runtime_builder.worker_threads(worker_threads);
    }
    let tokio_runtime = Arc::new(
        runtime_builder
            .enable_all()
            .build()
            .map_err(|error| internal_error("building the Tokio runtime", error))?,
    );

    // -- configuration, user configuration --
    let default_db_paths = DbPaths::for_data_dir(&paths.data_dir);
    let mut db_paths = default_db_paths.clone();
    let (secrets_path, key_path, crc_policy) =
        ConfigService::load_startup_config(&db_paths.config_db).unwrap_or((None, None, None));

    // `default_collision_policy` rides along with the `user_config` load
    // rather than joining `load_startup_config` above: it is an
    // `app_config` key/value read like the CRC policy, but unlike the
    // vault paths it is not needed to decide *which* databases to open,
    // so it belongs with the ordinary settings read rather than with the
    // three values that steer bootstrap itself.
    let (user_config, stored_collision_policy) =
        if let Ok(cfg_db) = arclain_core::config::ConfigDb::open(&db_paths.config_db) {
            let cfg_conn = cfg_db.into_sqlite_db();
            cfg_conn
                .with_connection(|conn| {
                    UserConfig::ensure_table(conn)?;
                    let user_config = UserConfig::load(conn)?.unwrap_or_default();
                    // Deliberately not `?`: a failure reading this one
                    // key/value entry must not discard the `user_config`
                    // row that was just read successfully. That row
                    // decides the 7-Zip path, the HTTP proxy routing,
                    // and which plugins load; losing it to a missing
                    // collision policy would be wildly out of
                    // proportion, and the policy has its own documented
                    // fallback.
                    let collision_policy =
                        arclain_core::get_config(conn, arclain_core::COLLISION_POLICY_CONFIG_KEY)
                            .ok()
                            .flatten();
                    Ok((user_config, collision_policy))
                })
                .unwrap_or_default()
        } else {
            (UserConfig::default(), None)
        };
    let encrypted_crc_policy =
        crc_policy.unwrap_or_else(|| crate::settings::DEFAULT_ENCRYPTED_CRC_POLICY.to_string());
    let default_collision_policy =
        stored_collision_policy.unwrap_or_else(crate::settings::default_collision_policy_token);

    if let Some(secrets_path) = secrets_path {
        db_paths.secrets_db = secrets_path;
    }
    if let Some(key_path) = key_path {
        db_paths.key_file = Some(key_path);
    } else if let Ok(keyfile_env) = std::env::var("ARCLAIN_KEYFILE") {
        if !keyfile_env.trim().is_empty() {
            db_paths.key_file = Some(PathBuf::from(keyfile_env.trim()));
        }
    }

    if let Some(ref key_path) = db_paths.key_file {
        if !key_path.exists() {
            info!(
                "Master key file not found, generating new key at: {}",
                key_path.display()
            );
            let new_key = SecretsKey::generate();
            if let Err(error) = new_key.save_to_file(key_path) {
                warn!("Failed to save generated key file: {}", error);
            } else {
                info!("Master key file created successfully");
            }
        }
    }

    // -- backend selector and fallback --
    // A missing 7-Zip degrades the application; it does not stop it from
    // starting. The native backends list and index zip/rar/7z on their
    // own, `capabilities()`/`health()` report the reduced surface (zip
    // and rar read-only, "sevenzip" degraded), and every operation that
    // genuinely needs the CLI -- extract, create, convert -- checks for
    // it at invocation and fails with its own message naming the tool.
    // Refusing to boot made all of that unreachable.
    //
    // `SevenZipCli::detect` does not verify an *explicit* path exists
    // (it trusts the caller); the `exe_path().exists()` check below
    // closes that gap so a stale configured path is treated the same as
    // "not found" rather than silently accepted and failing later on
    // first use.
    let sevenzip_path_override = overrides
        .sevenzip_path
        .or_else(|| user_config.sevenzip_path.as_ref().map(PathBuf::from));
    let fallback_backend = match SevenZipCli::detect(sevenzip_path_override.as_deref()) {
        Ok(cli) if cli.exe_path().exists() => {
            info!("7-Zip CLI backend initialized as fallback");
            Some(cli)
        }
        _ => {
            warn!(
                "7-Zip not found (searched PATH and the configured sevenzip_path); \
                 archives remain browsable but read-only, and extract, create and \
                 convert are unavailable until 7-Zip is installed or its path is \
                 configured"
            );
            None
        }
    };

    let backend_selector = BackendSelector::new_native();
    info!("Backend selector initialized (native mode with fallbacks)");

    // Read-only PATH/well-known-install-location probe, added solely to
    // populate `capabilities()`/`health()`. Does not change any archive
    // operation's behavior: the per-format backend chain still resolves
    // unrar lazily at archive-open time, exactly as before this task.
    // Availability only, not a resolved path: `UnrarCli` exposes no path
    // accessor (unlike `SevenZipCli::exe_path()`) -- see
    // `session_store::compute_capabilities`'s doc comment.
    let unrar_available = UnrarCli::detect().is_some();

    // -- databases --
    let host_owns_plugin_network_routing = config.initial_plugin_network_routing.is_some();
    let mut core_services = CoreServices::new_with_plugin_network_routing(
        tokio_runtime.clone(),
        config.initial_plugin_network_routing,
    );
    if !host_owns_plugin_network_routing {
        core_services
            .async_http_client
            .apply_proxy_routing(None, effective_plugin_proxy_map(&user_config));
    }
    info!("Initialized HTTP client proxy settings");

    let mut pass_rules: Vec<PassRule> = Vec::new();
    let mut startup_password_rule_upgrades = 0usize;
    let mut content_cache: Option<Arc<ContentCache>> = None;
    let mut resource_manager: Option<Arc<ResourceManager>> = None;
    let mut dbs: Option<ConfigDbs> = None;
    let mut database_ready = false;

    if let Some(ref key_path) = db_paths.key_file {
        if let Ok(key) = SecretsKey::load_from_file(key_path) {
            match open_databases(&db_paths, &key) {
                Ok(opened_dbs) => {
                    if let Err(error) = core_services.init_db_services(&opened_dbs, &db_paths) {
                        warn!("Failed to initialize DB services: {}", error);
                    } else {
                        database_ready = true;

                        // -- content cache, resource manager --
                        if let Some(cache_svc) = core_services.cache_service.clone() {
                            // The blob store must share a root with the cache
                            // *index* this same profile opened above -- i.e.
                            // the resolved `paths`, never `core_services.
                            // cache_dir`, which re-derives the OS-conventional
                            // location and silently ignores `paths_override`.
                            // (For a default bootstrap the two are the same
                            // directory.) A split root means the index
                            // references blobs a differently-rooted store
                            // does not have, and every other profile's
                            // bootstrap reconciles -- deletes from -- the one
                            // shared OS-conventional store.
                            let cache_dir = paths.cache_dir.clone();
                            let resource_config = ResourceConfig {
                                fallback_dir: Some(paths.cache_dir.join("resources")),
                                ..Default::default()
                            };
                            match initialize_resource_services(
                                cache_dir,
                                cache_svc as Arc<dyn arclain_core::CacheIndex>,
                                resource_config,
                            ) {
                                Ok((cache, manager)) => {
                                    content_cache = Some(cache);
                                    resource_manager = Some(manager);
                                    info!("Content cache initialized via Services");
                                }
                                Err(error) => warn!("Failed to initialize content cache: {error}"),
                            }
                        }

                        if let Ok(rules) = opened_dbs.secrets.list_pass_rules() {
                            pass_rules = rules
                                .into_iter()
                                .map(|rule| PassRule {
                                    name: rule.name,
                                    pattern: rule.pattern,
                                    password: rule.password,
                                    priority: rule.priority,
                                    enabled: rule.enabled,
                                })
                                .collect();
                            startup_password_rule_upgrades =
                                upgrade_narrow_auto_saved_rules(&opened_dbs, &mut pass_rules);
                        }
                    }

                    // `ensure_default_rules` and `sync_rules` both seed the
                    // *same* organization_rules table when it is empty, with
                    // DIFFERENT payloads ("DLsite Standard" vs "DLSite
                    // Archive") -- whichever runs first wins, so the order
                    // here is load-bearing and must match the original
                    // `AppState::new`/`sync_configuration` order exactly:
                    // `ensure_default_rules` first, then `sync_rules`. Both
                    // run whenever `open_databases` succeeded, regardless of
                    // whether `init_db_services` (above) did -- matching the
                    // original code, which called them unconditionally in
                    // this same position, outside that branch.
                    if let Err(error) = arclain_core::config::database::ensure_default_rules(
                        &opened_dbs.config_pool,
                    ) {
                        warn!("Failed to organize default rules: {}", error);
                    }
                    if let Err(error) =
                        arclain_core::config::sync::sync_rules(&opened_dbs.config_pool)
                    {
                        warn!("Failed to sync organization rules: {}", error);
                    }
                    // Title filters: seeds system replacements and primes
                    // the in-memory cache -- ported from `AppState::
                    // sync_configuration`, this crate's now-sole caller.
                    if let Err(error) =
                        arclain_core::utilities::title_filter::init(&opened_dbs.config_pool)
                    {
                        warn!("Failed to initialize title filters: {}", error);
                    }

                    dbs = Some(opened_dbs);
                }
                Err(error) => warn!("Failed to open databases: {}", error),
            }
        }
    }

    // -- checksum service --
    let checksum_db_path = db_paths
        .config_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("checksum.sqlite");
    let checksum_service = match ChecksumService::open(&checksum_db_path) {
        Ok(service) => {
            let _ = service.recover_pending();
            Some(Arc::new(service))
        }
        Err(error) => {
            warn!("Failed to init checksum service: {}", error);
            None
        }
    };
    core_services.checksum_service = checksum_service.clone();

    // -- plugin manager, plugin service injection, plugin scheduler --
    info!("Initializing plugin system");
    info!("Using plugins directory: {}", paths.plugins_dir.display());
    let plugin_settings = user_config.get_all_plugin_settings();
    let mut plugin_manager: Option<Arc<SyncMutex<PluginManager>>> = None;
    let mut plugin_event_scheduler = None;

    if let Ok(mut manager) = PluginManager::new(paths.plugins_dir.clone(), plugin_settings) {
        manager.init().ok();

        // Reconcile every newly-discovered plugin's default-enabled state
        // (`register_prepared_plugin` always starts a plugin enabled)
        // against whatever `ArclainApp::set_plugin_enabled` last
        // persisted -- see `runtime::settings_ops::run_set_plugin_enabled`'s
        // own doc comment for why that write is always a full snapshot
        // of every plugin's actual enabled state, never an accumulated
        // diff: `None` (the column has never been written -- a fresh
        // install, or one where no plugin has ever been explicitly
        // toggled) leaves every plugin at its natural default, but once
        // `Some(_)` exists it is trusted completely, so a plugin absent
        // from it is disabled here, including one added to the plugins
        // directory after the last save (a newly-appeared plugin default-
        // disabled behind an explicit persisted allowlist is the safer
        // failure mode for anything that can reach the network/archive
        // data, not a bug).
        if user_config.enabled_plugins.is_some() {
            let persisted_enabled = user_config.get_enabled_plugins();
            for item in manager.list_plugins() {
                if !persisted_enabled.contains(&item.id) {
                    let _ = manager.disable_plugin(&item.id);
                }
            }
        }

        #[cfg(feature = "gameta")]
        if let Some(library_service) = core_services.library_service.clone() {
            manager.set_library_service(library_service);
        }
        if let Some(ref cache) = content_cache {
            manager.set_content_cache(cache.clone());
        }
        if let Some(ref resource_mgr) = resource_manager {
            manager.set_resource_manager(resource_mgr.clone());
        }
        manager.set_async_http_client(core_services.async_http_client.clone());
        if let Some(ref gameta_client) = core_services.gameta_client {
            manager.set_gameta_client(gameta_client.clone());
        }
        // Active context (the live per-tab bridge) intentionally not
        // wired here -- see this module's doc comment.

        plugin_event_scheduler = Some(manager.event_scheduler());

        // -- persisted state --
        if let Some(ui_service) = core_services.ui_service.clone() {
            match sync_plugin_top_tab_items(&ui_service, manager.get_all_top_tabs()) {
                Ok(0) => {}
                Ok(count) => info!("Synced {} plugin UI items to database", count),
                Err(error) => warn!("Failed to sync plugin UI items: {}", error),
            }
        }

        plugin_manager = Some(Arc::new(SyncMutex::new(manager)));
    }

    // Extraction's CLI-spawning seam: an explicit test override wins;
    // otherwise the real 7-Zip CLI this same bootstrap just detected
    // (`fallback_backend`) -- the exact binary every other archive
    // operation in this application already falls back to.
    let extract_runner: Arc<dyn crate::operations::extract::ExtractRunner> =
        config.extract_runner_override.unwrap_or_else(|| {
            Arc::new(crate::operations::extract::SevenZipRunner::new(
                fallback_backend.clone(),
            ))
        });

    // Read before `paths` moves into `AppRuntime`'s struct literal below.
    let materialization_root = paths.materialization_dir();
    let materialization_ttl = config
        .materialization_lease_ttl_override
        .unwrap_or(crate::materialization::DEFAULT_LEASE_TTL);

    let session = SessionStore {
        core_services: Arc::new(core_services),
        plugin_manager,
        content_cache,
        resource_manager,
        checksum_service,
        backend_selector,
        fallback_backend,
        unrar_available,
        plugin_event_scheduler,
        database_ready,
        mutable: parking_lot::RwLock::new(crate::settings::MutableSettings::new(
            user_config,
            pass_rules,
            encrypted_crc_policy,
            default_collision_policy,
            Some(db_paths),
            default_db_paths,
            dbs,
        )),
    };

    Ok(AppRuntime {
        paths,
        startup_password_rule_upgrades,
        session,
        operations: OperationRegistry::new(),
        archive_sessions: Arc::new(crate::archive::ArchiveSessionStore::new()),
        challenges: ChallengeWaiters::new(),
        archive_backend_override: config.archive_backend_override,
        extract_runner,
        materialization: crate::materialization::MaterializationStore::new(
            materialization_root,
            materialization_ttl,
        )?,
        // Set moments later by `ArclainApp::bootstrap`, once the cleanup
        // task is actually spawned -- see the field's own doc comment.
        cleanup_task_handle: parking_lot::Mutex::new(None),
        settings_write_lock: tokio::sync::Mutex::new(()),
        cache_maintenance_lock: parking_lot::Mutex::new(()),
        shut_down: std::sync::atomic::AtomicBool::new(false),
        plugin_sessions: crate::plugins::PluginSessionStore::new(),
        active_archive_session: crate::plugins::ActiveArchiveSession::new(),
        tokio_runtime: super::RuntimeOwner::new(tokio_runtime),
    })
}

/// Refresh the plugin top-tab rows of `ui_items` from the plugins' own
/// declarations, one row per enabled plugin's top tab
/// (`plugin:{plugin_id}:{tab_id}`, toolbar region, `plugins` group).
/// Returns how many rows were synced.
///
/// This runs on every launch, so it writes only the columns the plugin
/// declaration owns -- identity, labelling and dispatch wiring. The
/// arrangement of a row (`visible`, `sort_order`, `display_mode`) is
/// the user's, stored between launches by `ArclainApp::save_ui_items`
/// and the layout editors; the values built here only *seed* a row the
/// first time it appears (visible, at the tab's declared priority, in
/// the default display mode). See `UiService::sync_host_items`.
///
/// Plugin text is untrusted (WASM, bounded only by the runtime's ~1 MiB
/// whole-result quota), and the layout editor saves a whole region back
/// through `save_ui_items`, which refuses any text field over
/// [`crate::layout::MAX_UI_ITEM_TEXT_BYTES`]. A row stored over that
/// bound would therefore fail every later save of its region -- a
/// plugin bricking the layout editor with a long caption. So the sync
/// enforces the same bound the user path does: `label`/`icon` are
/// display text and are truncated to it (the tab still appears), while
/// a tab whose *identity* (`id`/`action_data`, both derived from the
/// declared tab id) cannot fit is skipped with a warning -- truncating
/// an id could collide with another row, and the live top-tab strip
/// renders from the plugin manager directly, so the tab itself keeps
/// working; it just gets no arrangeable row.
///
/// Deliberately not behind `settings_write_lock`: this runs during
/// bootstrap, before any application handle (and therefore any
/// concurrent facade writer) exists.
fn sync_plugin_top_tab_items(
    ui_service: &arclain_core::services::UiService,
    top_tabs: Vec<(String, arclain_plugins::types::TopTabConfig)>,
) -> anyhow::Result<usize> {
    /// Truncates plugin-declared display text to the largest char
    /// boundary at or under the user write path's per-field bound.
    fn clamp_text(value: String) -> String {
        let bound = crate::layout::MAX_UI_ITEM_TEXT_BYTES;
        if value.len() <= bound {
            return value;
        }
        let mut end = bound;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value[..end].to_string()
    }

    let mut ui_items = Vec::with_capacity(top_tabs.len());
    for (plugin_id, tab) in top_tabs {
        let id = format!("plugin:{}:{}", plugin_id, tab.id);
        let action_data = format!("{}:{}", plugin_id, tab.id);
        if id.len() > crate::layout::MAX_UI_ITEM_TEXT_BYTES
            || action_data.len() > crate::layout::MAX_UI_ITEM_TEXT_BYTES
        {
            warn!(
                "Skipping the toolbar row of a top tab of plugin {:?}: its declared id is \
                 too long to store",
                plugin_id
            );
            continue;
        }
        ui_items.push(UiItem {
            id,
            region: UiRegion::Toolbar,
            group_id: Some("plugins".to_string()),
            label: clamp_text(tab.label),
            icon: Some(clamp_text(tab.icon)),
            visible: true,
            sort_order: tab.priority as i32,
            display_mode: DisplayMode::IconAndText,
            action_type: ActionType::Plugin,
            action_data: Some(action_data),
        });
    }
    if !ui_items.is_empty() {
        ui_service.sync_host_items(&ui_items)?;
    }
    Ok(ui_items.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Ported from `crates/ui/src/core/state/init.rs`'s own test module,
    // along with `initialize_resource_services` itself.
    struct EmptyCacheIndex;

    impl arclain_core::CacheIndex for EmptyCacheIndex {
        fn upsert(
            &self,
            _key: &str,
            _product_id: Option<&str>,
            _content_hash: &str,
            _source_url: Option<&str>,
            _cache_type: arclain_core::CacheType,
            _size_bytes: Option<i64>,
        ) -> anyhow::Result<i64> {
            Ok(1)
        }

        fn get(&self, _key: &str) -> anyhow::Result<Option<arclain_core::CacheEntry>> {
            Ok(None)
        }

        fn has(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn delete(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn delete_by_pattern(&self, _pattern: &str) -> anyhow::Result<usize> {
            Ok(0)
        }

        fn update_last_accessed(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn startup_uses_one_resource_config_for_cache_and_manager() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_dir = temp.path().join("cache");
        let fallback_dir = cache_dir.join("resources");
        let mut config = ResourceConfig {
            fallback_dir: Some(fallback_dir.clone()),
            ..ResourceConfig::default()
        };
        config.cache_limits.max_object_bytes = 17;
        config.cache_limits.min_free_space_bytes = 0;

        let (cache, manager) =
            initialize_resource_services(cache_dir, Arc::new(EmptyCacheIndex), config).unwrap();

        assert_eq!(cache.limits().max_object_bytes, 17);
        assert_eq!(manager.config().cache_limits.max_object_bytes, 17);
        assert_eq!(manager.config().fallback_dir.as_ref(), Some(&fallback_dir));
    }

    /// A `UiService` over a real (temp-file) config database, its
    /// tables created and seeded the same way a launch creates them.
    fn temp_ui_service(temp: &tempfile::TempDir) -> arclain_core::services::UiService {
        let db_path = temp.path().join("config.sqlite");
        drop(arclain_core::config::ConfigDb::open(&db_path).expect("create the config database"));
        arclain_core::services::UiService::new(
            arclain_db::DieselPool::new(&db_path).expect("pool over the config database"),
        )
    }

    /// One enabled plugin declaring one top tab labelled `label`, in
    /// the shape `PluginManager::get_all_top_tabs` reports.
    fn one_top_tab(label: &str) -> Vec<(String, arclain_plugins::types::TopTabConfig)> {
        vec![(
            "demo".to_string(),
            arclain_plugins::types::TopTabConfig {
                id: "main".to_string(),
                label: label.to_string(),
                icon: "PUZZLE_PIECE".to_string(),
                badge: None,
                priority: 100,
            },
        )]
    }

    /// The layout editor loads a whole region and saves it back whole
    /// through `save_ui_items`, whose validation refuses any text field
    /// over `MAX_UI_ITEM_TEXT_BYTES`. A plugin-declared label is only
    /// bounded by the plugin runtime's ~1 MiB whole-result quota, so an
    /// unclamped sync would store a row that makes *every subsequent
    /// region save* fail -- a plugin bricking the toolbar editor with a
    /// long caption. The sync must clamp what it stores to the same
    /// bound the user path enforces, and the row must still appear
    /// (truncated), not be dropped.
    #[test]
    fn top_tab_sync_clamps_plugin_text_so_the_region_stays_saveable() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = temp_ui_service(&temp);

        // Multibyte text (3 bytes per char) whose length crosses the
        // bound mid-character, so a byte-index truncation would panic
        // or split a code point -- the clamp must land on a char
        // boundary at or under the bound.
        let oversized_label = "ラベル".repeat(crate::layout::MAX_UI_ITEM_TEXT_BYTES / 3);
        assert!(oversized_label.len() > crate::layout::MAX_UI_ITEM_TEXT_BYTES);
        let mut tabs = one_top_tab(&oversized_label);
        tabs[0].1.icon = "ア".repeat(crate::layout::MAX_UI_ITEM_TEXT_BYTES);

        assert_eq!(sync_plugin_top_tab_items(&service, tabs).unwrap(), 1);

        let row = {
            let mut items = service
                .list_items(UiRegion::Toolbar)
                .expect("list the toolbar");
            items.retain(|item| item.id == "plugin:demo:main");
            items
                .pop()
                .expect("the synced top-tab row must still appear")
        };
        assert!(
            row.label.len() <= crate::layout::MAX_UI_ITEM_TEXT_BYTES,
            "the stored label must respect the user write path's bound"
        );
        assert!(
            oversized_label.starts_with(&row.label) && !row.label.is_empty(),
            "the clamp is a truncation, not a replacement"
        );
        let icon = row.icon.clone().expect("the icon survives, clamped");
        assert!(icon.len() <= crate::layout::MAX_UI_ITEM_TEXT_BYTES);

        // The brick check itself: the whole region, exactly as the
        // layout editor reloads and re-saves it, passes the same
        // validation `save_ui_items` runs.
        let region: Vec<crate::layout::UiItemDto> = service
            .list_items(UiRegion::Toolbar)
            .expect("list the toolbar")
            .into_iter()
            .map(crate::layout::UiItemDto::from)
            .collect();
        crate::layout::items_to_core(crate::layout::UiRegionDto::Toolbar, region)
            .expect("a synced region must remain saveable by the layout editor");
    }

    /// `id`/`action_data` carry the tab's identity and dispatch wiring,
    /// so an oversized one cannot be truncated (a truncated id could
    /// collide and a truncated action_data would misdispatch). A tab
    /// whose identity cannot fit the row bound is skipped -- the live
    /// top-tab strip still renders it from the plugin manager directly;
    /// it just gets no arrangeable row -- and, critically, it cannot
    /// poison the region for the layout editor.
    #[test]
    fn a_top_tab_whose_identity_cannot_fit_a_row_is_skipped() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = temp_ui_service(&temp);

        let mut tabs = one_top_tab("Demo");
        tabs[0].1.id = "t".repeat(crate::layout::MAX_UI_ITEM_TEXT_BYTES + 1);

        assert_eq!(
            sync_plugin_top_tab_items(&service, tabs).unwrap(),
            0,
            "an unstorable tab is skipped, not stored oversized"
        );
        assert!(
            service
                .list_items(UiRegion::Toolbar)
                .expect("list the toolbar")
                .iter()
                .all(|item| item.id.len() <= crate::layout::MAX_UI_ITEM_TEXT_BYTES),
            "no oversized identity may reach the table"
        );
    }

    /// The startup sync runs on *every* launch, so it must only write
    /// the columns the plugin declaration owns (label, icon, dispatch
    /// wiring) and never the user's own arrangement of the row --
    /// visibility, position, display mode -- which `save_ui_items` and
    /// the layout editors store between launches.
    #[test]
    fn top_tab_sync_preserves_user_arrangement_and_applies_plugin_renames() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = temp_ui_service(&temp);
        let synced_row = |service: &arclain_core::services::UiService| {
            service
                .list_items(UiRegion::Toolbar)
                .expect("list the toolbar")
                .into_iter()
                .find(|item| item.id == "plugin:demo:main")
                .expect("the synced top-tab row")
        };

        // First launch: the sync creates the row, seeding the
        // arrangement from the tab's own declaration.
        assert_eq!(
            sync_plugin_top_tab_items(&service, one_top_tab("Demo")).unwrap(),
            1
        );
        let created = synced_row(&service);
        assert!(created.visible);
        assert_eq!(created.sort_order, 100);
        assert_eq!(created.label, "Demo");
        assert_eq!(created.display_mode, DisplayMode::IconAndText);

        // The user hides the tab, moves it, and switches its display
        // mode -- the row exactly as `save_ui_items` stores those edits.
        let mut arranged = created.clone();
        arranged.visible = false;
        arranged.sort_order = 5;
        arranged.display_mode = DisplayMode::TextOnly;
        service.upsert_items(&[arranged]).unwrap();

        // Next launch: the plugin renamed its tab. The rename lands;
        // the user's arrangement survives.
        assert_eq!(
            sync_plugin_top_tab_items(&service, one_top_tab("Demo (renamed)")).unwrap(),
            1
        );
        let after = synced_row(&service);
        assert_eq!(
            after.label, "Demo (renamed)",
            "the plugin-declared label is the sync's to refresh"
        );
        assert_eq!(after.icon.as_deref(), Some("PUZZLE_PIECE"));
        assert_eq!(after.action_type, ActionType::Plugin);
        assert_eq!(after.action_data.as_deref(), Some("demo:main"));
        assert!(!after.visible, "a launch must not unhide what the user hid");
        assert_eq!(
            after.sort_order, 5,
            "a launch must not move what the user arranged"
        );
        assert_eq!(
            after.display_mode,
            DisplayMode::TextOnly,
            "a launch must not reset the user's display mode"
        );
    }
}
