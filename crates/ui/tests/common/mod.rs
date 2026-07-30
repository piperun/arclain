//! Shared test scaffolding for arclain_ui integration tests.
//!
//! Each test file in `tests/` is its own crate compile, so anything
//! reusable must live in `tests/common/mod.rs` (Cargo's special rule:
//! the `mod.rs` form prevents `common` from being treated as a
//! standalone test binary).
//!
//! Use `create_test_shared_state()` for dispatcher tests that only
//! need a minimal `SharedState`. Use `TestContext` for richer
//! integration tests that also need a `PageNavigator`,
//! `OrganizationFeature`, and `egui::Context`.

#![allow(dead_code)] // helpers are imported selectively per test file

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::services::OrganizationService;
use arclain_core::{open_databases, DbPaths, UserConfig};
use arclain_db::SecretsKey;
use arclain_ui::core::navigation::PageNavigator;
use arclain_ui::core::services::Services;
use arclain_ui::core::state::AppState;
use arclain_ui::features::organization::OrganizationFeature;
use arclain_ui::shared::image_assets::ImageAssetStore;
use arclain_ui::shared::theme::AppTheme;
use arclain_ui::shared::SharedState;
use arclain_widgets::Toaster;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};

fn create_test_runtime() -> Runtime {
    // Integration tests run concurrently, and each SharedState owns a runtime.
    // Runtime::new() creates one worker per logical CPU, so a parallel test
    // binary that creates many SharedStates can start hundreds of Tokio
    // workers. Two workers still exercise task hand-off and concurrent
    // blocking jobs without overwhelming the scheduler.
    Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("create test runtime")
}

/// Build a minimal `SharedState` suitable for dispatcher unit tests.
///
/// `dbs` is `None`, so any dispatcher that touches the DB pool will
/// take its no-DB error branch — useful for asserting "given a
/// missing service, the dispatcher sets `state.error` rather than
/// panicking." For dispatcher tests that need real persistence,
/// extend this helper with an in-memory diesel pool (not yet built).
///
/// Requires a 7z binary on PATH; the test backend uses it as the
/// fallback. Tests on systems without 7z will panic at this helper.
pub fn create_test_shared_state() -> SharedState {
    let runtime = create_test_runtime();
    let services = Arc::new(Services::new(runtime));

    let app_state = AppState {
        user_config: UserConfig::default(),
        pass_rules: vec![],
        backend_selector: BackendSelector::new_native(),
        fallback_backend: SevenZipCli::detect(None).expect("7z executable not found for tests"),
        encrypted_crc_policy: "on_open".to_string(),
        db_paths: None,
        dbs: None,
        signals: arclain_ui::core::signals::AppSignals::new(),
    };

    let signals = app_state.signals.clone();

    let plugin_ui_jobs = arclain_ui::features::plugins::application::PluginUiJobs::new(
        services.plugin_manager.clone(),
        services.tokio_runtime.clone(),
    );
    let image_assets = ImageAssetStore::without_source(services.tokio_runtime.clone());
    SharedState {
        app_state: Arc::new(Mutex::new(app_state)),
        services,
        theme: AppTheme::new(false),
        toaster: Arc::new(Mutex::new(Toaster::new())),
        refresh_requests: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        plugin_ui_jobs,
        plugin_sessions: arclain_ui::features::plugins::application::PluginSessions::new(),
        image_assets,
        signals,
        facade: None,
        operation_origins: arclain_ui::core::operation_bridge::OperationOrigins::new(),
        materialization_actions: arclain_ui::core::operation_bridge::MaterializationActions::new(),
        external_open_leases: arclain_ui::core::operation_bridge::ExternalOpenLeases::new(),
    }
}

/// Bootstraps a real `ArclainApp` against an isolated temp directory
/// and attaches it to a minimal `SharedState` — what a dispatcher test
/// needs once the surface it exercises reads through the application
/// facade rather than a service handle.
///
/// Also primes the canonical chrome-item signals from the freshly
/// bootstrapped application, exactly the way `state/init.rs` does at
/// startup, so a test behaves like a running app: the layout editors and
/// the Interface page read those signals, and without the priming they
/// would see an empty layout no user would ever be shown.
///
/// The returned `TempDir` MUST stay alive for the duration of the test:
/// dropping it deletes the databases the facade has open.
///
/// Deliberately a second copy of the library's own
/// `test_support::bootstrap_test_facade`: that module is
/// `#[cfg(test)]`-private to `crates/ui` and unreachable from an
/// integration test, which compiles as its own crate against the public
/// API only (its doc comment says the same from the other side).
pub fn create_test_shared_state_with_facade() -> (TempDir, SharedState) {
    let temp = tempfile::tempdir().expect("create tempdir for the test facade");
    let app = arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
        paths_override: Some(arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        }),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap the test facade");

    let mut shared = create_test_shared_state();
    shared
        .app_state
        .lock()
        .reload_ui_config(&app, &shared.services.tokio_runtime);
    shared.facade = Some(app);
    (temp, shared)
}

/// Richer test context for archive-browser / navigation / organization
/// integration tests. Bundles the `SharedState` with a navigator, an
/// org feature, and an `egui::Context`.
pub struct TestContext {
    pub shared: SharedState,
    pub navigator: PageNavigator,
    pub org_feature: OrganizationFeature,
    pub egui_ctx: egui::Context,
}

impl TestContext {
    pub fn new() -> Self {
        let shared = create_test_shared_state();
        Self {
            org_feature: OrganizationFeature::new(&shared),
            shared,
            navigator: PageNavigator::new(),
            egui_ctx: egui::Context::default(),
        }
    }
}

/// Build a `SharedState` backed by real (temp-file) databases with
/// schemas applied. The returned `TempDir` MUST stay alive for the
/// duration of the test — dropping it deletes the temp directory and
/// invalidates the open SQLite handles.
///
/// Wires:
///   * `app_state.dbs` to the full `ConfigDbs` returned by
///     `open_databases` (includes config + cache pools, secrets, and
///     metadata store).
///   * `services.config_db` and `services.organization_service` to
///     wrappers over the config pool.
///   * Skips `services.library_service`, `services.cache_service`,
///     `services.config_service`, `services.checksum_service`,
///     `services.gameta_client`, `services.ui_service` — none of the MVU
///     dispatchers under test reach for those. Add them here when a
///     future test needs them.
///
/// No facade, so nothing that reads through `ArclainApp` works here:
/// chrome-layout and interface-settings dispatchers want
/// [`create_test_shared_state_with_facade`] instead.
///
/// The databases are empty (schema only, no rows) so happy-path
/// tests can stage their own fixtures via the service APIs and
/// observe round-trip behavior.
pub fn create_test_shared_state_with_dbs() -> (TempDir, SharedState) {
    let temp = tempfile::tempdir().expect("create tempdir for test DBs");
    let paths = DbPaths {
        config_db: temp.path().join("config.sqlite"),
        cache_db: temp.path().join("metadata.sqlite"),
        secrets_db: temp.path().join("pass.redb"),
        key_file: None,
    };
    let key = SecretsKey::generate();
    let dbs = open_databases(&paths, &key).expect("open test databases");

    let runtime = create_test_runtime();
    let mut services = Services::new(runtime);
    // arclain_ui::Services wraps CoreServices via Deref-only, so
    // db-backed fields live on `.core`.
    services.core.config_db = Some(Arc::new(dbs.config.clone()));
    services.core.organization_service =
        Some(Arc::new(OrganizationService::new(dbs.config_pool.clone())));
    let services = Arc::new(services);

    let app_state = AppState {
        user_config: UserConfig::default(),
        pass_rules: vec![],
        backend_selector: BackendSelector::new_native(),
        fallback_backend: SevenZipCli::detect(None).expect("7z executable not found for tests"),
        encrypted_crc_policy: "on_open".to_string(),
        db_paths: Some(paths),
        dbs: Some(dbs),
        signals: arclain_ui::core::signals::AppSignals::new(),
    };
    let signals = app_state.signals.clone();

    let plugin_ui_jobs = arclain_ui::features::plugins::application::PluginUiJobs::new(
        services.plugin_manager.clone(),
        services.tokio_runtime.clone(),
    );
    let image_assets = ImageAssetStore::without_source(services.tokio_runtime.clone());
    let shared = SharedState {
        app_state: Arc::new(Mutex::new(app_state)),
        services,
        theme: AppTheme::new(false),
        toaster: Arc::new(Mutex::new(Toaster::new())),
        refresh_requests: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        plugin_ui_jobs,
        plugin_sessions: arclain_ui::features::plugins::application::PluginSessions::new(),
        image_assets,
        signals,
        facade: None,
        operation_origins: arclain_ui::core::operation_bridge::OperationOrigins::new(),
        materialization_actions: arclain_ui::core::operation_bridge::MaterializationActions::new(),
        external_open_leases: arclain_ui::core::operation_bridge::ExternalOpenLeases::new(),
    };

    (temp, shared)
}
