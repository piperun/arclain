//! Helpers shared by this crate's unit tests.
//!
//! Only compiled under `cfg(test)`, and only reachable from `crates/ui`'s
//! own unit tests -- integration tests under `crates/ui/tests/` have
//! their own `common` module, and `arclain_app`'s `tests/support` is not
//! reachable from here.

use tempfile::TempDir;

use crate::core::signals::AppSignals;
use crate::core::AppState;

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

/// Bootstraps a real [`arclain_app::ArclainApp`] against an isolated
/// temp profile: real SQLite/redb files, a real vault, nothing touching
/// the developer's own profile.
///
/// Seeds a dummy 7-Zip executable and points `sevenzip_path` at it
/// first. `bootstrap` never runs the executable -- it only checks the
/// configured path exists -- but detection is unconditional and would
/// otherwise depend on whatever 7-Zip the machine running the test
/// happens to have on `PATH`.
pub fn bootstrap_test_facade(temp: &TempDir) -> arclain_app::ArclainApp {
    let paths = arclain_app::AppPaths {
        config_dir: temp.path().join("config"),
        data_dir: temp.path().join("data"),
        cache_dir: temp.path().join("cache"),
        log_dir: temp.path().join("logs"),
        plugins_dir: temp.path().join("plugins"),
    };

    let sevenzip_path = temp.path().join(sevenzip_exe_name());
    std::fs::write(
        &sevenzip_path,
        b"not a real binary, only its path is checked",
    )
    .expect("write dummy 7-Zip executable");

    arclain_app::ArclainApp::bootstrap_with_overrides(
        arclain_app::BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: None,
            archive_backend_override: None,
            extract_runner_override: None,
            materialization_lease_ttl_override: None,
            materialization_cleanup_interval_override: None,
        },
        arclain_app::BootstrapOverrides {
            sevenzip_path: Some(sevenzip_path),
        },
    )
    .expect("bootstrap a test facade")
}

/// Builds frontend state without unpacking any application-owned
/// services, exactly the way `AppState::new` does at startup.
///
/// Deliberately stops there: it does **not** seed the settings signals.
/// They are left at their placeholder so a test can choose whether to
/// fill them, and so a test about what happens when that fill *fails*
/// has a real un-filled state to observe.
pub fn app_state_from_facade(_facade: &arclain_app::ArclainApp) -> AppState {
    let signals = AppSignals::new();
    AppState { signals }
}
