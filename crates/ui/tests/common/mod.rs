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
use arclain_core::UserConfig;
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
