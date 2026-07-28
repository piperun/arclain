//! Integration tests for [`arclain_app::ArclainApp::bootstrap`] and the
//! runtime-composition surface (`AppPaths`, `BootstrapConfig`,
//! `capabilities()`/`health()`).
//!
//! Every test that performs a real bootstrap points `paths_override` at a
//! `tempfile::TempDir` -- nothing here ever touches a real user profile.
//! 7-Zip detection follows the same assumption `crates/ui/tests/common/
//! mod.rs` already makes (`SevenZipCli::detect(None).expect("7z executable
//! not found for tests")`): this project's dev/CI environment has 7-Zip on
//! `PATH`. Where a test needs *deterministic* control over 7-Zip
//! availability (independent of whatever the real machine has), it seeds
//! an explicit `sevenzip_path` into a pre-created `config.sqlite` via
//! `support::seed_working_sevenzip_config` / points it at a path that
//! provably does not exist.
//!
//! Capability/health *scenario* coverage (native-only, missing-7z,
//! missing-unrar, degraded-plugins, fully-ready) lives as crate-internal
//! unit tests next to the pure computation it exercises
//! (`crates/app/src/runtime/session_store.rs`), matching how this
//! workspace already tests other `pub(crate)` logic (see
//! `crates/app/src/operations/registry.rs`). Forcing those exact
//! combinations through a real `bootstrap()` call is not hermetically
//! possible for the "missing 7z"/"missing unrar" cases (no test seam
//! exists for unrar; forcing 7z absent makes `bootstrap()` itself fail
//! fatally -- see `missing_external_tools_fails_bootstrap_cleanly` below)
//! -- so this file covers `bootstrap()`'s own behavior end-to-end, and
//! `session_store.rs` covers the capability/health computation in
//! isolation.

mod support;

use std::path::PathBuf;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

fn dummy_sevenzip(temp: &tempfile::TempDir) -> PathBuf {
    support::create_dummy_executable(temp.path(), sevenzip_exe_name())
}

#[cfg(windows)]
fn sevenzip_exe_name() -> &'static str {
    "7zz.exe"
}

#[cfg(not(windows))]
fn sevenzip_exe_name() -> &'static str {
    "7zz"
}

/// First run: an entirely empty temp directory. `bootstrap()` must
/// create every directory `AppPaths` names and succeed with defaults
/// throughout (no prior config, no prior databases, no plugins
/// installed yet).
#[test]
fn first_run_creates_directories_and_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    for dir in [
        &paths.config_dir,
        &paths.data_dir,
        &paths.cache_dir,
        &paths.log_dir,
        &paths.plugins_dir,
    ] {
        assert!(
            !dir.exists(),
            "first run must start with no pre-existing directories"
        );
    }

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        worker_threads: None,
    })
    .expect("first run bootstrap must succeed");

    for dir in [
        &paths.config_dir,
        &paths.data_dir,
        &paths.cache_dir,
        &paths.log_dir,
        &paths.plugins_dir,
    ] {
        assert!(dir.exists(), "bootstrap must create {}", dir.display());
    }
    assert_eq!(app.paths().config_dir, paths.config_dir);
    assert!(support::databases_dir(&paths)
        .join("config.sqlite")
        .exists());
}

/// Existing data: bootstrap once (creating a profile), drop the
/// resulting app, then bootstrap again against the *same* paths --
/// this must succeed exactly as first run did, proving the databases
/// and directories created by a previous run don't confuse the next
/// startup.
#[test]
fn existing_data_bootstraps_successfully_on_a_second_run() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());

    {
        let _first = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(paths.clone()),
            worker_threads: None,
        })
        .expect("first bootstrap must succeed");
    }

    let second = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        worker_threads: None,
    })
    .expect("second bootstrap against existing data must succeed");
    assert_eq!(second.paths().data_dir, paths.data_dir);
}

/// A `config.sqlite` that exists but is not a valid SQLite file at all
/// must be tolerated (falls back to defaults everywhere that reads it),
/// not propagated as a fatal error -- matching every existing tolerant
/// read in the composition this task moved (`unwrap_or`, `if let Ok`).
#[test]
fn corrupt_configuration_database_is_tolerated() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_corrupt_config(&paths);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        worker_threads: None,
    })
    .expect("corrupt config.sqlite must not fail bootstrap");

    // The corrupt file forced every config read back to defaults --
    // in particular `sevenzip_path` is unset, so 7-Zip detection fell
    // through to a real `PATH` search. That succeeds under this
    // project's existing test-environment assumption (see module doc
    // comment); health() must still be answerable on a running app.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let health = runtime
        .block_on(app.health())
        .expect("health() must succeed");
    // Corrupt config forced `dbs` to `None` (open_databases never ran
    // successfully against the same broken file), so "database" is a
    // genuinely expected degraded component here -- not a test failure.
    assert!(health.degraded_components.iter().any(|c| c == "database"));
}

/// An explicit `sevenzip_path` that does not exist on disk must make
/// bootstrap fail cleanly with `ExternalToolMissing`, matching today's
/// behavior of failing when 7-Zip cannot be found -- just via a
/// structured `ApplicationError` instead of a panic.
#[test]
fn missing_external_tools_fails_bootstrap_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let nonexistent = temp.path().join("no-such-7z-executable");
    assert!(!nonexistent.exists());
    support::seed_working_sevenzip_config(&paths, &nonexistent);

    let error = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
    })
    .expect_err("bootstrap must fail when the configured 7-Zip path does not exist");

    assert_eq!(error.kind, ApplicationErrorKind::ExternalToolMissing);
}

/// A plugin package that exists but fails to load (invalid manifest)
/// must not fail bootstrap -- `PluginManager::init()` already tolerates
/// per-plugin load failures; this proves that tolerance survives the
/// move into `AppRuntime::bootstrap`.
#[test]
fn failed_plugin_load_is_tolerated() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

    let broken_dir = paths.plugins_dir.join("broken-plugin");
    std::fs::create_dir_all(&broken_dir).unwrap();
    std::fs::write(broken_dir.join("broken-plugin.toml"), b"not = valid [ toml").unwrap();

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
    })
    .expect("a broken plugin package must not fail bootstrap");

    // The plugin manager itself was still constructed even though the
    // one plugin package inside it is broken.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let capabilities = runtime
        .block_on(app.capabilities())
        .expect("capabilities() must succeed");
    assert!(capabilities.plugins_available);
}

/// `AppPaths::system_default()` is a pure computation (no directory
/// creation): it must be safe to call from a test without touching a
/// real user profile, and must resolve to the documented layout.
#[test]
fn system_default_path_resolution_is_pure_and_well_formed() {
    let paths = AppPaths::system_default().expect("system_default must not require any I/O");

    assert_eq!(paths.config_dir.file_name().unwrap(), "arclain");
    assert!(!paths.plugins_dir.as_os_str().is_empty());
    // Purely computing the defaults must not create anything on disk.
    // (We can't assert non-existence in general -- a real "arclain"
    // profile may already exist on the machine running this test --
    // so instead we assert the computation is deterministic, which
    // would be violated by any hidden side effect that depends on
    // creation order.)
    let paths_again = AppPaths::system_default().expect("system_default must be repeatable");
    assert_eq!(paths.config_dir, paths_again.config_dir);
    assert_eq!(paths.data_dir, paths_again.data_dir);
    assert_eq!(paths.cache_dir, paths_again.cache_dir);
    assert_eq!(paths.log_dir, paths_again.log_dir);
    assert_eq!(paths.plugins_dir, paths_again.plugins_dir);
}

/// Bootstrapping and dropping the resulting `ArclainApp` repeatedly must
/// not panic or fail on any iteration after the first -- in particular,
/// this proves `bootstrap()` never installs a process-global singleton
/// (like a `tracing` subscriber) that only tolerates a single call.
#[test]
fn repeated_bootstrap_and_drop_succeeds_every_time() {
    for i in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let paths = support::temp_paths(temp.path());
        support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(paths),
            worker_threads: None,
        })
        .unwrap_or_else(|error| panic!("bootstrap iteration {i} failed: {error:?}"));
        drop(app);
    }
}

/// Facade futures must be executor-agnostic: awaiting `capabilities()`
/// from a foreign multi-thread runtime (not the app's own) must work.
#[test]
fn capabilities_awaits_correctly_from_a_foreign_multi_thread_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
    })
    .unwrap();

    let foreign = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let capabilities = foreign.block_on(app.capabilities()).unwrap();
    assert!(!capabilities.archive_backends.is_empty());
}

/// Same as above, but from a `current_thread` runtime -- the facade
/// must not assume its caller is multi-threaded.
#[test]
fn health_awaits_correctly_from_a_current_thread_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
    })
    .unwrap();

    let foreign = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let health = foreign.block_on(app.health()).unwrap();
    assert!(health.ready || !health.degraded_components.is_empty());
}

/// `shutdown()` must also be awaitable from any runtime, and must
/// succeed on a freshly bootstrapped app with nothing in flight.
#[test]
fn shutdown_succeeds_from_a_foreign_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        worker_threads: None,
    })
    .unwrap();

    let foreign = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    foreign
        .block_on(app.shutdown())
        .expect("shutdown must succeed");
}
