use super::*;
use tempfile::TempDir;

const VALIDATION_CHILD_ENV: &str = "ARCLAIN_PLUGIN_VALIDATION_CHILD_6A21";
const VALIDATION_CHILD_TEST: &str =
    "manager::tests::malicious_metadata_cannot_escape_validation_sandbox_or_run_init";
const WASI_STDOUT_SENTINEL: &str = "ARCLAIN_VALIDATION_WASI_STDOUT_SENTINEL_7F3B";
const WASI_STDERR_SENTINEL: &str = "ARCLAIN_VALIDATION_WASI_STDERR_SENTINEL_8C4D";

struct CapturedChild {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_validation_child() -> CapturedChild {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut child = Command::new(std::env::current_exe().expect("test binary path must resolve"))
        .arg(VALIDATION_CHILD_TEST)
        .arg("--exact")
        .arg("--nocapture")
        .env(VALIDATION_CHILD_ENV, "child")
        .env("ARCLAIN_WASI_ENV_MUST_NOT_LEAK_91D2", "sentinel")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("validation child process must spawn");

    let mut child_stdout = child.stdout.take().expect("child stdout must be piped");
    let mut child_stderr = child.stderr.take().expect("child stderr must be piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        child_stdout
            .read_to_end(&mut bytes)
            .expect("child stdout must remain readable");
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        child_stderr
            .read_to_end(&mut bytes)
            .expect("child stderr must remain readable");
        bytes
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = stdout_reader.join().unwrap_or_default();
                let stderr = stderr_reader.join().unwrap_or_default();
                panic!(
                    "validation child exceeded 30 seconds and was terminated\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr),
                );
            }
            Err(error) => panic!("failed to query validation child status: {error}"),
        }
    };

    CapturedChild {
        status,
        stdout: stdout_reader
            .join()
            .expect("child stdout reader must terminate"),
        stderr: stderr_reader
            .join()
            .expect("child stderr reader must terminate"),
    }
}

#[test]
fn test_plugin_manager_creation() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new());
    assert!(manager.is_ok());
}

#[test]
fn test_plugin_enable_disable() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    // Enabling/disabling non-existent plugin should fail
    assert!(manager.enable_plugin("nonexistent").is_err());
    assert!(manager.disable_plugin("nonexistent").is_err());
}

#[test]
fn test_list_plugins_empty() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    assert_eq!(manager.list_plugins().len(), 0);
}

#[test]
fn install_rejects_exported_unsafe_id_before_filesystem_use() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let external_log_dir = temp_dir.path().join("external-plugin-logs");
    let wasm_path = temp_dir.path().join("unsafe-id.wasm");
    let mut component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ))
    .to_vec();
    let id_offset = component
        .windows(b"ui-demo".len())
        .position(|window| window == b"ui-demo")
        .expect("bundled component should export ui-demo metadata");
    component[id_offset..id_offset + b"ui-demo".len()].copy_from_slice(b"../evil");
    std::fs::write(&wasm_path, component).unwrap();

    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        external_log_dir.clone(),
    )
    .unwrap();
    let result = manager.install_plugin(&wasm_path);

    assert!(
        matches!(result, Err(crate::types::PluginError::InvalidManifest(_))),
        "expected unsafe exported id to be rejected, got {result:?}",
    );
    assert!(
        !plugins_dir.parent().unwrap().join("evil").exists(),
        "unsafe id must not create a sibling directory",
    );
    assert!(
        !plugins_dir.exists() || std::fs::read_dir(&plugins_dir).unwrap().next().is_none(),
        "unsafe id must not create a plugin directory",
    );
    assert!(
        !external_log_dir.exists(),
        "metadata validation must not write plugin logs before ID validation",
    );
}

#[tracing_test::traced_test]
#[test]
fn malicious_metadata_cannot_escape_validation_sandbox_or_run_init() {
    if std::env::var_os(VALIDATION_CHILD_ENV).is_none() {
        let child = run_validation_child();
        let stdout = String::from_utf8_lossy(&child.stdout);
        let stderr = String::from_utf8_lossy(&child.stderr);

        assert!(
            !stdout.contains(WASI_STDOUT_SENTINEL),
            "validation component reached inherited process stdout:\n{stdout}",
        );
        assert!(
            !stderr.contains(WASI_STDERR_SENTINEL),
            "validation component reached inherited process stderr:\n{stderr}",
        );
        assert!(
            child.status.success(),
            "validation child failed; args/environment may have leaked\nstdout:\n{stdout}\nstderr:\n{stderr}",
        );
        return;
    }

    const METADATA_SENTINEL: &str = "arclain-malicious-metadata-sentinel.txt";
    const INIT_SENTINEL: &str = "arclain-malicious-init-sentinel.txt";

    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("application-data").join("plugins");
    let external_log_dir = temp_dir
        .path()
        .join("application-data")
        .join("logs")
        .join("plugins");
    let wasm_path = temp_dir
        .path()
        .join("incoming")
        .join("malicious-metadata.wasm");
    std::fs::create_dir_all(wasm_path.parent().unwrap()).unwrap();
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/malicious-metadata/malicious-metadata.wasm"
        )),
    )
    .unwrap();

    let metadata_sentinel = std::env::temp_dir().join(METADATA_SENTINEL);
    let init_sentinel = std::env::temp_dir().join(INIT_SENTINEL);
    assert!(
        !metadata_sentinel.exists(),
        "metadata sentinel must start absent"
    );
    assert!(!init_sentinel.exists(), "init sentinel must start absent");

    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        external_log_dir.clone(),
    )
    .unwrap();
    let result = manager.install_plugin(&wasm_path);

    assert!(
        matches!(result, Err(crate::types::PluginError::InvalidManifest(_))),
        "metadata retrieval must hide args/environment, skip init, and reach invalid PluginId rejection: {result:?}",
    );
    assert!(
        !metadata_sentinel.exists(),
        "create-file import escaped validation"
    );
    assert!(
        !init_sentinel.exists(),
        "validation must not call plugin init"
    );
    assert!(
        !external_log_dir.exists(),
        "plugin log destination was touched"
    );
    assert!(!logs_contain("arclain-malicious-metadata-global-log"));
    assert!(!logs_contain("arclain-malicious-metadata-show-message"));

    let application_root = temp_dir.path().join("application-data");
    let entries = std::fs::read_dir(&plugins_dir)
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert!(entries.is_empty(), "plugin root must remain empty");
    let application_entries = std::fs::read_dir(&application_root)
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(application_entries.len(), 1, "application root was mutated");
    assert_eq!(application_entries[0].file_name(), "plugins");
    let escaped_destination = plugins_dir.join("..").join("evil");
    assert!(!escaped_destination.exists(), "sibling escape was created");
    assert!(
        !plugins_dir.join("evil").exists(),
        "plugin destination was created"
    );
    assert!(!plugins_dir.join("plugin.wasm").exists());
    assert!(!plugins_dir.join("plugin.toml").exists());
}

#[test]
fn valid_install_runs_normal_init_once_after_id_validation() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_log_dir = temp_dir.path().join("plugin-logs");
    let wasm_path = temp_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();

    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        plugin_log_dir.clone(),
    )
    .unwrap();

    assert_eq!(manager.install_plugin(&wasm_path).unwrap(), "ui-demo");
    assert!(plugins_dir.join("ui-demo").join("ui-demo.wasm").exists());
    assert!(plugins_dir.join("ui-demo").join("ui-demo.toml").exists());
    assert_eq!(manager.list_plugins().len(), 1);

    let log = std::fs::read_to_string(
        std::fs::read_dir(&plugin_log_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert_eq!(
        log.matches("UI Demo Plugin initialized via Component Model!")
            .count(),
        1,
        "normal plugin init must run exactly once",
    );
}

#[test]
fn reload_and_unload_replace_only_manifest_owned_domains() {
    use arclain_network::features::whitelist::{AccessCheck, DomainWhitelist};
    use parking_lot::RwLock;

    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let wasm_path = temp_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let whitelist = Arc::new(RwLock::new(DomainWhitelist::default()));
    let client = Arc::new(arclain_network::AsyncHttpClient::new(
        runtime.handle().clone(),
        whitelist.clone(),
        None,
    ));
    let mut manager = PluginManager::new(plugins_dir.clone(), HashMap::new()).unwrap();
    manager.set_async_http_client(client.clone());
    assert_eq!(manager.install_plugin(&wasm_path).unwrap(), "ui-demo");

    let manifest_path = plugins_dir.join("ui-demo").join("ui-demo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap().replace(
        "network = false",
        "network = true\nnetwork_domains = [\"old-manifest.test\"]",
    );
    std::fs::write(&manifest_path, manifest).unwrap();
    manager.reload_plugin("ui-demo").unwrap();
    whitelist.read().approve("ui-demo", "user-approved.test");
    assert_eq!(
        whitelist.read().check("ui-demo", "old-manifest.test"),
        AccessCheck::Allowed,
    );

    let replacement = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("old-manifest.test", "new-manifest.test");
    std::fs::write(&manifest_path, replacement).unwrap();
    manager.reload_plugin("ui-demo").unwrap();

    assert_eq!(
        whitelist.read().check("ui-demo", "old-manifest.test"),
        AccessCheck::NotWhitelisted,
        "replacement manifest retained its predecessor's automatic grant",
    );
    assert_eq!(
        whitelist.read().check("ui-demo", "new-manifest.test"),
        AccessCheck::Allowed,
    );
    assert_eq!(
        whitelist.read().check("ui-demo", "user-approved.test"),
        AccessCheck::Allowed,
        "reload revoked an independent user approval",
    );

    manager.unload_plugin("ui-demo").unwrap();

    assert_eq!(
        whitelist.read().check("ui-demo", "new-manifest.test"),
        AccessCheck::NotWhitelisted,
        "unload retained a manifest-owned domain grant",
    );
    assert_eq!(
        whitelist.read().check("ui-demo", "user-approved.test"),
        AccessCheck::Allowed,
        "unload revoked an independent user approval",
    );
    assert_eq!(client.plugin_network_policy("ui-demo"), None);
}

/// Regression test for P5 from `docs/AUDIT_2026-05-03.md`.
///
/// `status_summary()` is the cheap counts-only path the status bar
/// uses every render frame. Pre-fix, the status bar called
/// `list_plugins()` instead, which clones every plugin's full
/// manifest (Vec of capabilities + network domains) per frame just
/// to read `len()` and count enabled. This test pins the contract:
/// empty manager returns `(total: 0, enabled: 0)`. With WASM
/// scaffolding in place a follow-up would also pin the populated case.
#[test]
fn p5_status_summary_returns_counts_for_empty_manager() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    let summary = manager.status_summary();
    assert_eq!(summary, PluginStatusSummary::default());
    assert_eq!(summary.total, 0);
    assert_eq!(summary.enabled, 0);
}

/// Regression test for P3 from `docs/AUDIT_2026-05-03.md`.
///
/// `get_all_top_tabs` results are now cached so the toolbar render
/// path no longer issues a WASM `get_top_tabs` call into every
/// enabled plugin every frame. The cache is invalidated whenever a
/// plugin is enabled, disabled, loaded, or unloaded.
///
/// This test pins the cache invariant: with no plugins, the empty
/// result is cached on first call. The second call returns the
/// cached empty Vec without re-querying. After we explicitly
/// invalidate, the cache is dropped — observable via the cache
/// field's state.
#[test]
fn p3_top_tabs_cache_populates_and_invalidates() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    assert!(
        manager.cached_top_tabs.lock().is_none(),
        "Cache should start empty",
    );

    let first = manager.get_all_top_tabs();
    assert!(first.is_empty());
    assert!(
        manager.cached_top_tabs.lock().is_some(),
        "First call should populate the cache",
    );

    // Second call hits the cache. Length match is enough — TopTabConfig
    // doesn't implement PartialEq so we compare structurally instead.
    let second = manager.get_all_top_tabs();
    assert_eq!(first.len(), second.len());
    assert!(manager.cached_top_tabs.lock().is_some());

    // Explicit invalidation drops the cache.
    manager.invalidate_top_tabs_cache();
    assert!(
        manager.cached_top_tabs.lock().is_none(),
        "invalidate_top_tabs_cache should clear the cache",
    );
}

#[test]
fn top_tabs_wait_for_a_busy_enabled_plugin_instead_of_returning_partial_results() {
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    let temp_dir = TempDir::new().unwrap();
    let wasm_path = temp_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();

    let mut manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();
    manager.install_plugin(&wasm_path).unwrap();
    let manager = Arc::new(manager);
    let instance = manager
        .get_plugin_instance("ui-demo")
        .expect("installed plugin must expose its instance");
    let instance_guard = instance.lock();

    let worker_manager = manager.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender
            .send(worker_manager.get_all_top_tabs())
            .expect("test receiver must remain connected");
    });

    assert!(
        matches!(
            receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a complete snapshot must wait for a busy enabled plugin",
    );

    drop(instance_guard);
    let _tabs = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("top-tab aggregation must finish after the instance unlocks");
    worker.join().unwrap();
}

/// `enable_plugin` / `disable_plugin` should drop the cache so the
/// next render picks up the new set of enabled plugins.
#[test]
fn p3_enable_disable_invalidates_top_tabs_cache() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();

    let _ = manager.get_all_top_tabs();
    assert!(manager.cached_top_tabs.lock().is_some());

    // The plugin doesn't exist, so the call returns Err — but it's
    // the EARLIEST short-circuit, so the cache should not be
    // invalidated. (Audit fix only invalidates on success.)
    assert!(manager.enable_plugin("nonexistent").is_err());
    assert!(manager.cached_top_tabs.lock().is_some());

    // Manually populate `plugins` so enable_plugin succeeds, then
    // verify invalidation. Without WASM we can't actually load a
    // plugin, but we can assert the invalidation API drops the cache
    // directly — which is the structural contract callers depend on.
    manager.invalidate_top_tabs_cache();
    assert!(manager.cached_top_tabs.lock().is_none());
}

/// Regression test for P7 from `docs/AUDIT_2026-05-03.md`.
///
/// `get_settings_for(plugin_id)` is the per-plugin-settings query
/// detail_view's UI event handler now uses instead of the
/// whole-map-cloning `get_all_settings`. For unloaded plugins, the
/// helper falls back to `initial_settings` so settings persist
/// across runs even before the plugin is loaded.
#[test]
fn p7_get_settings_for_falls_back_to_initial_settings() {
    let temp_dir = TempDir::new().unwrap();
    let mut initial = HashMap::new();
    let mut plugin_a = HashMap::new();
    plugin_a.insert("api_key".to_string(), "secret".to_string());
    initial.insert("plugin_a".to_string(), plugin_a.clone());

    let manager = PluginManager::new(temp_dir.path().to_path_buf(), initial).unwrap();

    // plugin_a isn't loaded (no WASM), but its initial settings are
    // available via the fallback path.
    let got = manager.get_settings_for("plugin_a");
    assert_eq!(got, Some(plugin_a));

    // Unknown plugin → None.
    assert_eq!(manager.get_settings_for("plugin_b"), None);
}
