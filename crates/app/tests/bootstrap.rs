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
//! `crates/app/src/operations/registry.rs`). Forcing every one of those
//! combinations through a real `bootstrap()` call is still not
//! hermetically possible (no test seam exists for unrar), so
//! `session_store.rs` covers the computation in isolation and this file
//! covers `bootstrap()`'s own behavior end-to-end -- including the
//! missing-7z case, which reaches `capabilities()`/`health()` now that
//! bootstrap degrades instead of failing (see
//! `a_missing_sevenzip_degrades_capabilities_instead_of_failing_bootstrap`).

mod support;

use std::path::PathBuf;

use arclain_app::error::ApplicationErrorKind;
use arclain_app::{AppPaths, ArclainApp, BootstrapConfig, BootstrapOverrides};

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

#[test]
fn bootstrap_override_takes_precedence_without_being_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let persisted_path =
        support::create_dummy_executable(&temp.path().join("persisted"), sevenzip_exe_name());
    let override_path =
        support::create_dummy_executable(&temp.path().join("override"), sevenzip_exe_name());
    support::seed_working_sevenzip_config(&paths, &persisted_path);

    let app = ArclainApp::bootstrap_with_overrides(
        BootstrapConfig {
            paths_override: Some(paths),
            ..Default::default()
        },
        BootstrapOverrides {
            sevenzip_path: Some(override_path.clone()),
        },
    )
    .expect("the application-owned fixture override must satisfy 7-Zip detection");

    let runtime = tokio::runtime::Runtime::new().expect("create test runtime");
    let capabilities = runtime
        .block_on(app.capabilities())
        .expect("read application capabilities");
    let sevenzip = capabilities
        .external_tools
        .iter()
        .find(|tool| tool.tool == "7z")
        .expect("7-Zip capability");
    assert_eq!(
        sevenzip.resolved_path.as_deref(),
        Some(override_path.as_path())
    );

    let settings = runtime
        .block_on(app.settings())
        .expect("read persisted settings through the facade");
    assert_eq!(
        settings.archive.sevenzip_path.as_deref(),
        Some(persisted_path.as_path()),
        "the process-local override must not replace the persisted setting"
    );
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
        ..Default::default()
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
            ..Default::default()
        })
        .expect("first bootstrap must succeed");
    }

    let second = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths.clone()),
        ..Default::default()
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
        ..Default::default()
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

/// A 7-Zip that cannot be resolved must degrade the application, not
/// stop it from starting: browsing an archive never invokes the CLI, so
/// bootstrap succeeds and reports the reduced surface through
/// `capabilities()`/`health()` instead. An explicit `sevenzip_path` that
/// does not exist on disk is the deterministic seam for "no 7-Zip"
/// (`SevenZipCli::detect` trusts an explicit path, so bootstrap's own
/// existence check is what rejects it) and needs no control over the
/// machine's real `PATH`.
#[test]
fn a_missing_sevenzip_degrades_capabilities_instead_of_failing_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let nonexistent = temp.path().join("no-such-7z-executable");
    assert!(!nonexistent.exists());
    support::seed_working_sevenzip_config(&paths, &nonexistent);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("a configured 7-Zip path that does not exist must not fail bootstrap");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let capabilities = runtime
        .block_on(app.capabilities())
        .expect("capabilities() must succeed");

    let sevenzip = capabilities
        .external_tools
        .iter()
        .find(|tool| tool.tool == "7z")
        .expect("7-Zip capability");
    assert!(!sevenzip.available);
    assert_eq!(sevenzip.resolved_path, None);

    // Absence is reported honestly all the way down: with no CLI tier to
    // union in, zip and rar advertise only their read-only native
    // backends, while the native 7z backend stays full-featured.
    let backend = |name: &str| {
        capabilities
            .archive_backends
            .iter()
            .find(|b| b.backend == name)
            .unwrap_or_else(|| panic!("{name} backend capability"))
            .clone()
    };
    assert!(backend("zip").can_list && !backend("zip").can_create);
    assert!(backend("rar").can_list && !backend("rar").can_create);
    assert!(backend("7z").can_create);

    let health = runtime
        .block_on(app.health())
        .expect("health() must succeed");
    assert!(health.degraded_components.iter().any(|c| c == "sevenzip"));
    assert!(
        !health.ready,
        "extract/create/convert are unavailable, so the app is degraded -- just not dead"
    );
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
        ..Default::default()
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
            ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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
        ..Default::default()
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

/// Guards the invariant `crate::materialization::run_cleanup_task`'s
/// `Weak<AppRuntime>` fix exists to establish: `AppRuntime` (and
/// everything it composes -- the plugin manager, database pools, caches,
/// every worker thread) must actually drop once the last `ArclainApp`
/// clone is gone. Before that fix, the cleanup task held a strong
/// `Arc<AppRuntime>` in a loop with nothing to end it (`AppRuntime` -> its
/// own runtime -> the task -> the same `Arc<AppRuntime>`, a closed cycle),
/// so `AppRuntime` was never dropped no matter how many `ArclainApp`
/// clones a caller dropped -- a real, permanent resource leak in every
/// bootstrapped application.
///
/// `AppRuntime` itself is `pub(crate)`, unreachable from this integration
/// test -- observed indirectly instead: `archive_backend_override`
/// installs a backend whose own `Drop` impl flips a shared flag. Since
/// nothing in this test ever calls `start_open_archive` (no session is
/// ever opened), the *only* place holding a reference to that backend is
/// `AppRuntime` itself, so the flag becoming true is proof positive that
/// the whole composed `AppRuntime` was actually dropped.
#[test]
fn app_runtime_actually_drops_once_every_arclain_app_clone_is_gone() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct DropObserverBackend {
        dropped: Arc<AtomicBool>,
    }
    impl Drop for DropObserverBackend {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    impl arclain_core::ArchiveBackend for DropObserverBackend {
        fn name(&self) -> &str {
            "drop-observer"
        }
        fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
            arclain_core::archive::BackendCapabilities::read_only()
        }
        fn identify(
            &self,
            _path: &std::path::Path,
        ) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
            unimplemented!("never called: this test never opens an archive")
        }
        fn list(
            &self,
            _path: &std::path::Path,
            _password: Option<&str>,
        ) -> anyhow::Result<arclain_core::ArchiveInfo> {
            unimplemented!()
        }
        fn extract_all(
            &self,
            _path: &std::path::Path,
            _dest: &std::path::Path,
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_files(
            &self,
            _path: &std::path::Path,
            _dest: &std::path::Path,
            _files: &[String],
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_directory(
            &self,
            _path: &std::path::Path,
            _dest: &std::path::Path,
            _dir_path: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn recompress_7z(
            &self,
            _source: &std::path::Path,
            _dest_7z: &std::path::Path,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_files(&self, _archive: &std::path::Path, _files: &[PathBuf]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn create_archive(
            &self,
            _dest: &std::path::Path,
            _files: &[PathBuf],
            _format: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn read_text_file(
            &self,
            _archive: &std::path::Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        fn delete_files(
            &self,
            _archive: &std::path::Path,
            _files: &[String],
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_or_update_file_from_str(
            &self,
            _archive: &std::path::Path,
            _path_in_archive: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn convert_to_7z(
            &self,
            _source: &arclain_core::Archive,
            _dest: &std::path::Path,
            _temp_dir: &std::path::Path,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn crc32_of_entry(
            &self,
            _archive: &std::path::Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let dropped = Arc::new(AtomicBool::new(false));
    let backend: Arc<dyn arclain_core::ArchiveBackend> = Arc::new(DropObserverBackend {
        dropped: dropped.clone(),
    });

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        archive_backend_override: Some(backend),
        // Short enough that the cleanup task is definitely alive and has
        // actually ticked at least once by the time this test drops the
        // app below -- so a pass here cannot be a coincidence of the task
        // never having started polling yet.
        materialization_cleanup_interval_override: Some(std::time::Duration::from_millis(5)),
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !dropped.load(Ordering::SeqCst),
        "sanity: must not be dropped yet -- the app is still alive"
    );

    drop(app);

    // Bounded poll: `RuntimeOwner::shutdown_now`'s teardown (reached via
    // `AppRuntime`'s own `Drop`, once the last `Arc<AppRuntime>` reference
    // is actually gone) does not block, so the observer's own drop can
    // land a moment after this call returns, not necessarily before it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !dropped.load(Ordering::SeqCst) {
        if std::time::Instant::now() >= deadline {
            panic!(
                "AppRuntime (and everything it owns) was never dropped after the last \
                 ArclainApp clone went away -- the cleanup task's Arc<AppRuntime> may still be \
                 holding an unbreakable cycle"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// `ensure_default_rules` and `sync_rules` both seed the organization
/// rules table only when it is empty, with *different* product rules
/// ("DLsite Standard" vs. "DLSite Archive") -- whichever runs first
/// wins. The original `AppState::new`/`sync_configuration` always ran
/// `ensure_default_rules` first; this proves `bootstrap()` still does,
/// so a fresh install always gets "DLsite Standard", never
/// `sync_rules`'s "DLSite Archive" payload, ever again by accident.
///
/// It also proves the mod-manager layout reaches a real first run.
/// Nothing else can put one in the database -- a layout is data a rule
/// carries and the rules editor does not write one -- so an archive
/// holding several mods produces sibling folders only if this rule is
/// seeded here.
#[test]
fn first_run_seeds_ensure_default_rules_payload_not_sync_rules_payload() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("first run bootstrap must succeed");

    let legacy = app
        .take_legacy_composition()
        .expect("take_legacy_composition must succeed before shutdown");
    let dbs = legacy
        .dbs
        .expect("databases must have opened successfully on a first run");
    let rules = arclain_core::config::database::list_org_rules(&dbs.config_pool)
        .expect("list organization rules");

    let names: Vec<&str> = rules.iter().map(|rule| rule.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["DLsite Standard", "Mod Manager Layout"],
        "the seed set of whichever seeder ran first, and only that one's"
    );

    let product = &rules[0];
    assert_eq!(
        product.trigger.filename_pattern.as_deref(),
        Some(r"(RJ|VJ|BJ)\d+"),
        "this is ensure_default_rules's pattern, not sync_rules's \\[(RJ|BJ|VJ)\\d+\\]"
    );
    assert_eq!(product.actions.layout.name, "Game");
    assert!(
        product.trigger.metadata_source.is_none(),
        "sync_rules's product rule is the one that keys on a metadata source"
    );

    let mods = &rules[1];
    assert!(
        matches!(
            mods.actions.layout.outputs,
            arclain_core::features::organization::layout::OutputSelector::PerDirectoryContaining {
                ref marker
            } if marker == "modinfo.ini"
        ),
        "the mod-manager layout must reach a first run: {:?}",
        mods.actions.layout.outputs
    );
    assert_eq!(mods.trigger.has_file.as_deref(), Some("modinfo.ini"));
}

/// A plugins directory that cannot be created must not fail bootstrap,
/// only degrade plugin loading. Simulated by planting a regular file
/// where the plugins directory would go (`create_dir_all` then fails
/// outright) -- the same class of failure a root-owned system-install
/// plugins directory (`/usr/lib/arclain/plugins`, `Program Files\
/// Arclain\plugins`) produces for a non-root process, but reproducible
/// hermetically on any platform without real permission manipulation.
#[test]
fn uncreatable_plugins_dir_still_bootstraps_with_plugins_degraded() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    std::fs::write(&paths.plugins_dir, b"not a directory").unwrap();

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("an uncreatable plugins dir must not fail bootstrap");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let capabilities = runtime
        .block_on(app.capabilities())
        .expect("capabilities() must succeed");
    assert!(
        !capabilities.plugins_available,
        "plugin manager construction should have failed gracefully against a non-directory path"
    );
    let health = runtime
        .block_on(app.health())
        .expect("health() must succeed");
    assert!(health.degraded_components.iter().any(|c| c == "plugins"));
    assert!(
        !app.install_active_tab_bridge(|_| {
            panic!("a missing plugin runtime cannot invoke the fallback")
        })
        .expect("bridge setup must preserve degraded startup"),
        "bridge setup must report that there was no plugin runtime to wire"
    );
}

/// End-to-end companion to the deterministic `RuntimeOwner`-focused
/// regression tests in `arclain_app::runtime`'s own test module (which
/// directly prove the underlying mechanism, including checking the
/// background task's `JoinHandle` result -- something not possible
/// through the public API alone, since a panic inside a detached
/// spawned task is caught by Tokio and would not otherwise be observed
/// here). This test exercises the same shape through the public API:
/// dropping a facade future before it resolves, then dropping the
/// app's last clone from within an async context, must not hang or
/// bring down the test process.
#[test]
fn dropping_a_facade_future_mid_flight_then_the_app_does_not_panic() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .unwrap();

    let foreign = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    foreign.block_on(async {
        // At least one poll happens before a timeout this short can
        // possibly fire, which is enough to reach `ArclainApp::
        // dispatch`'s internal `spawn` onto the app's own runtime (the
        // spawn happens synchronously, before the first await point).
        // Timing out drops the future without ever seeing the result
        // -- "mid-flight" -- while the already-spawned background task
        // keeps running independently to completion regardless.
        let _ = tokio::time::timeout(std::time::Duration::from_micros(1), app.capabilities()).await;

        // Drop this test's own `ArclainApp` clone now -- possibly
        // before the detached background task has finished, leaving
        // it holding what may become the very last `Arc<AppRuntime>`
        // reference.
        drop(app);

        // Give the detached background task time to finish and drop
        // its own clone, from within one of the app's own runtime's
        // worker threads.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
}

/// After `shutdown()`, every other facade call must return a structured
/// error rather than silently spawning onto a runtime that may already
/// be tearing down.
#[test]
fn calling_a_facade_method_after_shutdown_returns_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(app.shutdown())
        .expect("shutdown must succeed");

    let error = runtime
        .block_on(app.capabilities())
        .expect_err("calling capabilities() after shutdown must fail");
    assert_eq!(error.kind, ApplicationErrorKind::Internal);
}

/// A second `shutdown()` call is a documented idempotent no-op, not an
/// error.
#[test]
fn shutting_down_twice_is_an_idempotent_no_op() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(app.shutdown())
        .expect("first shutdown must succeed");
    runtime
        .block_on(app.shutdown())
        .expect("second shutdown must also succeed (idempotent no-op)");
}

/// A clone of `ArclainApp` obtained before `shutdown()` is called on a
/// *different* clone must also observe the shut-down state -- the flag
/// lives in the shared `AppRuntime`, not per-clone.
#[test]
fn a_clone_outliving_shutdown_also_gets_the_error() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .unwrap();
    let clone = app.clone();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime
        .block_on(app.shutdown())
        .expect("shutdown on the original clone must succeed");
    drop(app);

    let error = runtime
        .block_on(clone.capabilities())
        .expect_err("a clone outliving shutdown must also see the shut-down state");
    assert_eq!(error.kind, ApplicationErrorKind::Internal);
}

/// The exact convention this crate's own docs prescribe -- await
/// `shutdown()`, then drop the last clone -- must not panic, even when
/// both happen from inside the same async context. Distinct from
/// `dropping_a_facade_future_mid_flight_then_the_app_does_not_panic`
/// above: that test drops the app *without* ever calling `shutdown()`.
/// This one calls `shutdown()` first, which in a real bootstrapped app
/// can (almost) never actually reclaim the runtime yet --
/// `SessionStore::core_services` is still alive for as long as the app
/// itself is -- and *that* is exactly the sequence an earlier version
/// of `RuntimeOwner::shutdown_now` handled unsafely: it gave up its own
/// protective `Arc` clone on the first (failing) `try_unwrap` and never
/// got it back, leaving `session`'s bare clone as the sole survivor, so
/// dropping the app immediately afterward reached `tokio::runtime::
/// Runtime`'s unprotected `Drop` directly instead of `RuntimeOwner`'s.
#[test]
fn shutdown_then_dropping_the_app_in_the_same_async_context_does_not_panic() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    support::seed_working_sevenzip_config(&paths, &dummy_sevenzip(&temp));
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .unwrap();

    let foreign = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    foreign.block_on(async {
        app.shutdown().await.expect("shutdown must succeed");
        drop(app);
    });
}

/// `health()`/`capabilities()` must reflect whether the 7-Zip
/// executable this instance detected at bootstrap is *still* there,
/// not a value frozen at bootstrap time. This is the half where a 7-Zip
/// that *was* present is deleted after a successful bootstrap; the half
/// where there was never one to detect is
/// `a_missing_sevenzip_degrades_capabilities_instead_of_failing_bootstrap`.
/// Bootstrap succeeds in both cases -- browsing an archive never invokes
/// the CLI, and the operations that need it check at invocation -- which
/// is why both are reachable here at all. Also proves 7-Zip is a
/// *required* component (unlike unrar): its removal clears `ready`.
#[test]
fn health_reflects_sevenzip_removed_after_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let sevenzip_path = dummy_sevenzip(&temp);
    support::seed_working_sevenzip_config(&paths, &sevenzip_path);
    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("bootstrap must succeed with the seeded dummy 7-Zip present");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let health_before = runtime.block_on(app.health()).unwrap();
    assert!(
        health_before.ready,
        "must be ready while the detected 7-Zip executable still exists"
    );

    std::fs::remove_file(&sevenzip_path).unwrap();

    let health_after = runtime.block_on(app.health()).unwrap();
    assert!(
        !health_after.ready,
        "7-Zip is a required component: its removal must clear readiness"
    );
    assert!(health_after
        .degraded_components
        .iter()
        .any(|c| c == "sevenzip"));

    let capabilities_after = runtime.block_on(app.capabilities()).unwrap();
    let sevenzip_tool = capabilities_after
        .external_tools
        .iter()
        .find(|t| t.tool == "7z")
        .unwrap();
    assert!(!sevenzip_tool.available);
}

/// Guards `support::databases_dir`'s doc comment, which claims it
/// mirrors `AppPaths::databases_dir`'s (crate-private, unreachable from
/// this external test crate) convention exactly. If the two ever
/// drifted apart, `seed_working_sevenzip_config` (which writes into
/// `support::databases_dir`) would be seeding a `config.sqlite`
/// bootstrap never actually reads, and 7-Zip detection would fall
/// through to a real `PATH` search instead of finding the seeded dummy
/// executable -- visible here as `capabilities()` reporting a
/// *different* resolved path (or bootstrap failing outright, on a
/// machine with no real 7-Zip on `PATH`) instead of the exact dummy
/// path this test seeded.
#[test]
fn paths_documented_layout_matches_test_support() {
    let temp = tempfile::tempdir().unwrap();
    let paths = support::temp_paths(temp.path());
    let sevenzip_path = dummy_sevenzip(&temp);
    support::seed_working_sevenzip_config(&paths, &sevenzip_path);

    let app = ArclainApp::bootstrap(BootstrapConfig {
        paths_override: Some(paths),
        ..Default::default()
    })
    .expect("bootstrap must succeed");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let capabilities = runtime.block_on(app.capabilities()).unwrap();
    let sevenzip_tool = capabilities
        .external_tools
        .iter()
        .find(|t| t.tool == "7z")
        .unwrap();
    assert_eq!(
        sevenzip_tool.resolved_path.as_deref(),
        Some(sevenzip_path.as_path()),
        "bootstrap must have read sevenzip_path from support::databases_dir()'s exact location"
    );
}
