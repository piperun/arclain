//! Integration test for `arclain_ui::core::app_lifecycle::
//! shutdown_facade_on_exit`.
//!
//! Before this fix, `eframe::App::on_exit` only saved tab state and
//! never called `ArclainApp::shutdown()` at all -- so `clear_all()`
//! and runtime teardown never ran in the shipped binary, and every
//! doc comment claiming otherwise was describing code that did not
//! exist. `shutdown_facade_on_exit` is the extracted, synchronous-
//! callback-safe function `on_exit` now calls; this test proves it
//! actually drives a *real* facade's `shutdown()` to completion, not
//! just that it compiles and type-checks against the facade type.
//!
//! Neither `common::create_test_shared_state()` nor `common::
//! create_test_shared_state_with_dbs()` builds a real facade (both
//! set `facade: None` -- see their doc comments), so this file
//! bootstraps one directly against a temp-directory-rooted
//! `AppPaths`, mirroring `crates/app/tests/support::temp_paths`
//! (that helper lives in `arclain_app`'s own test tree and isn't
//! reachable from an `arclain_ui` integration test).

mod common;

use std::path::Path;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};
use arclain_ui::core::app_lifecycle::shutdown_facade_on_exit;
use arclain_ui::shared::SharedState;

fn temp_paths(root: &Path) -> AppPaths {
    AppPaths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        log_dir: root.join("logs"),
        plugins_dir: root.join("plugins"),
    }
}

/// Bootstraps a real `ArclainApp` against an isolated temp directory.
/// Relies on 7-Zip being on `PATH`, the same assumption `crates/app/
/// tests/bootstrap.rs`'s `first_run_creates_directories_and_succeeds`
/// and `common::create_test_shared_state`'s own `SevenZipCli::detect`
/// call already make for this workspace's dev/CI environment.
fn bootstrap_real_facade(temp: &tempfile::TempDir) -> ArclainApp {
    ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(temp_paths(temp.path())),
        worker_threads: None,
        archive_backend_override: None,
        extract_runner_override: None,
        materialization_lease_ttl_override: None,
        materialization_cleanup_interval_override: None,
    })
    .expect("bootstrap a real ArclainApp for the shutdown-on-exit test")
}

/// Proves `shutdown_facade_on_exit` drives a real facade's async
/// `shutdown()` to completion from a synchronous call, by observing
/// the well-established post-shutdown contract: any further facade
/// call fails with `ApplicationErrorKind::Internal`. Mirrors
/// `arclain_app`'s own `calling_a_facade_method_after_shutdown_returns_
/// an_error` in `crates/app/tests/bootstrap.rs`, just entered through
/// the UI-side wrapper instead of calling `shutdown()` directly.
///
/// `facade` is cloned into `shared.facade` (as `SharedState::new`
/// does at real startup) while the original binding is kept alive so
/// the test can call `capabilities()` on it afterward -- `ArclainApp`
/// clones share one `Arc<AppRuntime>`, so shutting down through one
/// clone is observable through every other.
#[test]
fn shutdown_facade_on_exit_actually_shuts_the_facade_down() {
    let temp = tempfile::tempdir().unwrap();
    let facade = bootstrap_real_facade(&temp);

    let shared = SharedState {
        facade: Some(facade.clone()),
        ..common::create_test_shared_state()
    };

    shutdown_facade_on_exit(&shared);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(facade.capabilities())
        .expect_err("calling capabilities() after on_exit's shutdown must fail");
    assert_eq!(error.kind, ApplicationErrorKind::Internal);
}

/// `on_exit` runs unconditionally on every window close, including
/// every existing `arclain_ui` test harness (`facade: None`) and any
/// future embedding that hasn't wired a facade yet. Must be a silent
/// no-op, never a panic.
#[test]
fn shutdown_facade_on_exit_is_a_no_op_when_there_is_no_facade() {
    let shared = common::create_test_shared_state();
    shutdown_facade_on_exit(&shared);
}
