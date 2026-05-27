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
use arclain_core::services::{OrganizationService, UiService};
use arclain_core::{open_databases, DbPaths, UserConfig};
use arclain_db::SecretsKey;
use arclain_ui::core::navigation::PageNavigator;
use arclain_ui::core::services::Services;
use arclain_ui::core::state::AppState;
use arclain_ui::features::organization::OrganizationFeature;
use arclain_ui::shared::theme::AppTheme;
use arclain_ui::shared::SharedState;
use arclain_widgets::Toaster;
use eframe::egui;
use parking_lot::Mutex;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

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
    let runtime = Runtime::new().unwrap();
    let services = Arc::new(Services::new(runtime));

    let app_state = AppState {
        user_config: UserConfig::default(),
        pass_rules: vec![],
        backend_selector: BackendSelector::new_native(),
        fallback_backend: SevenZipCli::detect(None).expect("7z executable not found for tests"),
        last_entries: vec![],
        encrypted_crc_policy: "on_open".to_string(),
        db_paths: None,
        dbs: None,
        plugin_event_sender: None,
        pending_plugin_event: None,
        signals: arclain_ui::core::signals::AppSignals::new(),
    };

    let signals = app_state.signals.clone();

    SharedState {
        app_state: Arc::new(Mutex::new(app_state)),
        services,
        theme: AppTheme::new(false),
        toaster: Arc::new(Mutex::new(Toaster::new())),
        refresh_requests: Arc::new(Mutex::new(Vec::new())),
        pending_plugin_actions: Arc::new(Mutex::new(Vec::new())),
        signals,
    }
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
///   * `services.config_db`, `services.organization_service`,
///     `services.ui_service` to wrappers over the config pool.
///   * Skips `services.library_service`, `services.cache_service`,
///     `services.config_service`, `services.checksum_service`,
///     `services.gameta_client` — none of the MVU dispatchers under
///     test reach for those. Add them here when a future test needs
///     them.
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

    let runtime = Runtime::new().unwrap();
    let mut services = Services::new(runtime);
    // arclain_ui::Services wraps CoreServices via Deref-only, so
    // db-backed fields live on `.core`.
    services.core.config_db = Some(Arc::new(dbs.config.clone()));
    services.core.organization_service =
        Some(Arc::new(OrganizationService::new(dbs.config_pool.clone())));
    let ui_svc = Arc::new(UiService::new(dbs.config_pool.clone()));
    services.core.ui_service = Some(ui_svc.clone());
    let services = Arc::new(services);

    let app_state = AppState {
        user_config: UserConfig::default(),
        pass_rules: vec![],
        backend_selector: BackendSelector::new_native(),
        fallback_backend: SevenZipCli::detect(None).expect("7z executable not found for tests"),
        last_entries: vec![],
        encrypted_crc_policy: "on_open".to_string(),
        db_paths: Some(paths),
        dbs: Some(dbs),
        plugin_event_sender: None,
        pending_plugin_event: None,
        signals: arclain_ui::core::signals::AppSignals::new(),
    };
    let signals = app_state.signals.clone();

    // Mirror state/init.rs's signal-population step so tests behave
    // like a freshly-started app: the canonical item signals are
    // seeded from the (just-initialized) DB. The LayoutEditor
    // dispatcher reads these signals; without this priming, tests
    // would see empty signals and never populate state.items.
    if let Ok(items) = ui_svc.list_toolbar_items() {
        signals.toolbar_items.set(items);
    }
    if let Ok(items) = ui_svc.list_info_panel_items() {
        signals.info_panel_items.set(items);
    }
    if let Ok(items) = ui_svc.list_items(arclain_core::UiRegion::ContextMenu) {
        signals.context_menu_items.set(items);
    }

    let shared = SharedState {
        app_state: Arc::new(Mutex::new(app_state)),
        services,
        theme: AppTheme::new(false),
        toaster: Arc::new(Mutex::new(Toaster::new())),
        refresh_requests: Arc::new(Mutex::new(Vec::new())),
        pending_plugin_actions: Arc::new(Mutex::new(Vec::new())),
        signals,
    };

    (temp, shared)
}
