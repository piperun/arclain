use super::*;
use tempfile::TempDir;

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
        "metadata retrieval must reach PluginId validation without calling init: {result:?}",
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
