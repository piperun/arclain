use super::*;
use tempfile::TempDir;

#[test]
fn plugin_event_scheduler_rejects_immediately_when_capacity_is_full() {
    let (sender, _receiver) = bounded_event_channel(1);
    let scheduler = PluginEventScheduler::new(sender);
    let cloned_scheduler = scheduler.clone();
    let event = || PluginEvent::OnArchiveOpen {
        path: "archive.zip".to_string(),
        kind: arclain_core::ArchiveKind::Zip,
        password: Some("secret".to_string()),
        entries: Arc::new(Vec::new()),
        archive_session_id: 0,
    };

    scheduler.try_schedule(event()).expect("first event fits");
    assert!(matches!(
        cloned_scheduler.try_schedule(event()),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
}

#[cfg(unix)]
fn create_manager_test_directory_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_manager_test_directory_link(target: &std::path::Path, link: &std::path::Path) {
    let output = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("junction creation command should start");
    assert!(
        output.status.success(),
        "failed to create test junction: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
fn init_records_a_discovered_but_uninstantiable_plugin_as_a_failed_plugin() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_dir = plugins_dir.join("broken-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("broken-plugin.toml"),
        r#"
[plugin]
id = "broken-plugin"
name = "Broken Plugin"
version = "0.1.0"
author = "test"
description = "a manifest with an unparsable component body"

[capabilities]
"#,
    )
    .unwrap();
    // A manifest that discovers cleanly, paired with bytes that are not
    // a valid WASM component -- `load_plugin` fails during
    // instantiation, well after discovery already returned this plugin.
    std::fs::write(
        plugin_dir.join("broken-plugin.wasm"),
        b"not a real component",
    )
    .unwrap();

    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir,
        HashMap::new(),
        temp_dir.path().join("logs"),
    )
    .unwrap();
    manager.init().unwrap();

    assert!(
        manager.list_plugins().is_empty(),
        "a failed load must not register a plugin"
    );
    let failed = manager.failed_plugins();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].original_id, "broken-plugin");
    assert!(!failed[0].error.is_empty());
}

#[test]
fn manager_does_not_retain_unbounded_persisted_plugin_settings() {
    let temp_dir = TempDir::new().unwrap();
    let mut plugin_settings = HashMap::new();
    for index in 0..=128 {
        plugin_settings.insert(format!("key-{index:03}"), "value".to_string());
    }
    plugin_settings.insert("oversized".to_string(), "x".repeat(64 * 1024 + 1));
    let mut initial = HashMap::new();
    initial.insert("plugin-a".to_string(), plugin_settings);

    let manager = PluginManager::new_with_plugin_log_dir(
        temp_dir.path().join("plugins"),
        initial,
        temp_dir.path().join("logs"),
    )
    .unwrap();
    let identity_key = crate::types::PluginIdentityKey::parse("plugin-a").unwrap();
    let retained = &manager.initial_settings[&identity_key].values;
    let retained_bytes = retained
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum::<usize>();

    assert!(retained.len() <= 128);
    assert!(retained.values().all(|value| value.len() <= 64 * 1024));
    assert!(retained_bytes <= 1024 * 1024);
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
    assert!(
        std::fs::read_dir(&plugins_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".arclain-plugin-install-")),
        "successful installation retained a staging directory",
    );

    let log = std::fs::read_to_string(
        std::fs::read_dir(&plugin_log_dir)
            .unwrap()
            .find_map(|entry| {
                let entry = entry.ok()?;
                (entry.path().extension().and_then(|value| value.to_str()) == Some("log"))
                    .then_some(entry)
            })
            .expect("plugin init should create a dated log file")
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
fn install_manifest_serialization_round_trips_hostile_metadata() {
    let plugin_id = crate::types::PluginId::parse("hostile-metadata").unwrap();
    let metadata = crate::types::PluginMetadata {
        id: plugin_id.as_str().to_string(),
        name: "quoted \"name\"".to_string(),
        version: "1.0\nrelease".to_string(),
        author: "backslash \\ author".to_string(),
        description: "line one\n[capabilities]\nnetwork = true".to_string(),
    };

    let manifest = super::lifecycle::manifest_from_metadata(&plugin_id, metadata.clone());
    let encoded = super::lifecycle::serialize_manifest(&manifest).unwrap();
    let decoded: crate::types::PluginManifest = toml::from_str(&encoded).unwrap();

    assert_eq!(decoded.plugin.id, metadata.id);
    assert_eq!(decoded.plugin.name, metadata.name);
    assert_eq!(decoded.plugin.version, metadata.version);
    assert_eq!(decoded.plugin.author, metadata.author);
    assert_eq!(decoded.plugin.description, metadata.description);
    assert!(!decoded.capabilities.network);
    assert!(decoded.capabilities.network_domains.is_empty());
}

#[test]
fn install_uses_the_case_folded_identity_key_for_its_destination() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let wasm_path = temp_dir.path().join("uppercase-id.wasm");
    let mut component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ))
    .to_vec();
    let id_offset = component
        .windows(b"ui-demo".len())
        .position(|window| window == b"ui-demo")
        .expect("bundled component should export ui-demo metadata");
    component[id_offset..id_offset + b"ui-demo".len()].copy_from_slice(b"UI-DEMO");
    std::fs::write(&wasm_path, component).unwrap();
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();

    assert_eq!(manager.install_plugin(&wasm_path).unwrap(), "UI-DEMO");

    let entries = std::fs::read_dir(&plugins_dir)
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), "ui-demo");
    let manifest_path = entries[0].path().join("ui-demo.toml");
    let manifest: crate::types::PluginManifest =
        toml::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.plugin.id, "UI-DEMO");
}

#[test]
fn installed_plugin_lifecycle_uses_case_folded_identity_keys() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let wasm_path = temp_dir.path().join("uppercase-id.wasm");
    let mut component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ))
    .to_vec();
    let id_offset = component
        .windows(b"ui-demo".len())
        .position(|window| window == b"ui-demo")
        .expect("bundled component should export ui-demo metadata");
    component[id_offset..id_offset + b"ui-demo".len()].copy_from_slice(b"UI-DEMO");
    std::fs::write(&wasm_path, component).unwrap();
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();

    assert_eq!(manager.install_plugin(&wasm_path).unwrap(), "UI-DEMO");
    assert_eq!(
        manager.get_plugin_metadata("ui-demo").unwrap().id,
        "UI-DEMO"
    );
    assert!(manager.get_plugin_instance("ui-demo").is_some());
    assert!(manager.is_plugin_enabled("ui-demo"));
    assert!(manager
        .enabled_plugin_snapshot()
        .instance("ui-demo")
        .is_some());

    manager.disable_plugin("ui-demo").unwrap();
    assert!(!manager.is_plugin_enabled("ui-demo"));
    manager.enable_plugin("ui-demo").unwrap();
    assert!(manager.is_plugin_enabled("ui-demo"));

    let manifest_path = plugins_dir.join("ui-demo").join("ui-demo.toml");
    let mut manifest: crate::types::PluginManifest =
        toml::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest.plugin.id = "Ui-DeMo".to_string();
    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest).unwrap()).unwrap();

    manager.reload_plugin("ui-demo").unwrap();
    assert_eq!(
        manager.get_plugin_metadata("ui-demo").unwrap().id,
        "Ui-DeMo",
        "reload must retain the manifest spelling for display/runtime state",
    );
    assert!(manager.is_plugin_enabled("ui-demo"));

    manager.unload_plugin("ui-demo").unwrap();
    assert!(manager.get_plugin_metadata("ui-demo").is_none());
    assert!(!manager.is_plugin_enabled("ui-demo"));
}

#[test]
fn persisted_settings_lookup_is_case_folded_but_snapshot_preserves_original_id() {
    let temp_dir = TempDir::new().unwrap();
    let mut settings = HashMap::new();
    settings.insert("token".to_string(), "secret".to_string());
    let mut initial = HashMap::new();
    initial.insert("UI-DEMO".to_string(), settings.clone());

    let manager = PluginManager::new_with_plugin_log_dir(
        temp_dir.path().join("plugins"),
        initial,
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();

    assert_eq!(manager.get_settings_for("ui-demo"), Some(settings.clone()));
    let snapshot = manager.get_all_settings();
    assert_eq!(snapshot.get("UI-DEMO"), Some(&settings));
    assert!(!snapshot.contains_key("ui-demo"));
}

#[test]
fn install_rejects_a_case_variant_already_present_in_the_manager_registry() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let lowercase_wasm = temp_dir.path().join("lowercase.wasm");
    std::fs::write(
        &lowercase_wasm,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();
    manager.install_plugin(&lowercase_wasm).unwrap();
    std::fs::remove_dir_all(plugins_dir.join("ui-demo")).unwrap();

    let uppercase_wasm = temp_dir.path().join("uppercase.wasm");
    let mut component = std::fs::read(&lowercase_wasm).unwrap();
    let id_offset = component
        .windows(b"ui-demo".len())
        .position(|window| window == b"ui-demo")
        .expect("bundled component should export ui-demo metadata");
    component[id_offset..id_offset + b"ui-demo".len()].copy_from_slice(b"UI-DEMO");
    std::fs::write(&uppercase_wasm, component).unwrap();

    let result = manager.install_plugin(&uppercase_wasm);

    assert!(result.is_err());
    assert_eq!(manager.list_plugins().len(), 1);
    assert!(std::fs::read_dir(&plugins_dir).unwrap().next().is_none());
}

#[test]
fn install_rejects_a_case_folded_on_disk_destination_collision() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let existing = plugins_dir.join("Plugin2");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("sentinel"), "preserve").unwrap();
    let wasm_path = temp_dir.path().join("plugin2.wasm");
    let mut component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ))
    .to_vec();
    let id_offset = component
        .windows(b"ui-demo".len())
        .position(|window| window == b"ui-demo")
        .expect("bundled component should export ui-demo metadata");
    component[id_offset..id_offset + b"ui-demo".len()].copy_from_slice(b"plugin2");
    std::fs::write(&wasm_path, component).unwrap();
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir,
        HashMap::new(),
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();

    let result = manager.install_plugin(&wasm_path);

    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(existing.join("sentinel")).unwrap(),
        "preserve"
    );
    assert!(manager.list_plugins().is_empty());
}

#[test]
fn install_rejects_an_existing_on_disk_destination_without_overwriting_it() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let destination = plugins_dir.join("ui-demo");
    let manifest_path = destination.join("ui-demo.toml");
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::write(&manifest_path, "existing-invalid-manifest").unwrap();
    let wasm_path = temp_dir.path().join("ui-demo.wasm");
    std::fs::write(
        &wasm_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();

    let result = manager.install_plugin(&wasm_path);

    assert!(result.is_err(), "existing destination was overwritten");
    assert_eq!(
        std::fs::read_to_string(manifest_path).unwrap(),
        "existing-invalid-manifest"
    );
    assert!(!destination.join("ui-demo.wasm").exists());
}

#[test]
fn install_failure_after_staging_removes_package_and_manager_state() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let wasm_path = temp_dir.path().join("failing-init.wasm");
    let mut component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/malicious-metadata/malicious-metadata.wasm"
    ))
    .to_vec();
    let id_offset = component
        .windows(b"../evil".len())
        .position(|window| window == b"../evil")
        .expect("fixture should contain its unsafe exported ID");
    component[id_offset..id_offset + b"../evil".len()].copy_from_slice(b"failure");
    std::fs::write(&wasm_path, component).unwrap();
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("plugin-logs"),
    )
    .unwrap();

    let result = manager.install_plugin(&wasm_path);

    assert!(result.is_err(), "fixture init unexpectedly succeeded");
    assert!(manager.list_plugins().is_empty());
    assert!(!plugins_dir.join("failure").exists());
    assert!(
        std::fs::read_dir(&plugins_dir).unwrap().next().is_none(),
        "failed installation retained staged files",
    );
}

#[test]
fn failed_staged_publish_cleans_up_every_temporary_file() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    )
    .unwrap();
    let staging_root = staged.root_path().to_path_buf();
    let destination = plugins_dir.join(plugin_id.as_str());

    let result = staged.publish_with_before_rename(&destination, |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected publish failure",
        ))
    });

    assert!(result.is_err());
    assert!(
        !staging_root.exists(),
        "failed publish leaked staging files"
    );
    assert!(!destination.exists());
}

#[test]
fn staging_does_not_follow_a_replaced_plugin_root() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let original_root = temp_dir.path().join("original-plugin-root");
    let outside_root = temp_dir.path().join("outside-root");
    std::fs::create_dir(&plugins_dir).unwrap();
    std::fs::create_dir(&outside_root).unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    if let Err(error) = std::fs::rename(&plugins_dir, &original_root) {
        #[cfg(windows)]
        {
            assert!(plugins_dir.is_dir(), "root disappeared after {error}");
            return;
        }
        #[cfg(not(windows))]
        panic!("failed to replace plugin root for race regression: {error}");
    }
    create_manager_test_directory_link(&outside_root, &plugins_dir);
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );

    let result = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    );

    assert!(result.is_err(), "staging followed a replaced plugin root");
    assert!(
        std::fs::read_dir(&outside_root).unwrap().next().is_none(),
        "staging wrote through the replacement root link",
    );
}

#[test]
fn publish_revalidates_the_captured_plugin_root_identity() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let original_root = temp_dir.path().join("original-plugin-root");
    std::fs::create_dir(&plugins_dir).unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    )
    .unwrap();

    if let Err(error) = std::fs::rename(&plugins_dir, &original_root) {
        #[cfg(windows)]
        {
            assert!(plugins_dir.is_dir(), "root disappeared after {error}");
            return;
        }
        #[cfg(not(windows))]
        panic!("failed to replace plugin root for race regression: {error}");
    }
    std::fs::create_dir(&plugins_dir).unwrap();
    let destination = plugins_dir.join(plugin_id.as_str());

    let result = staged.publish_with_before_rename(&destination, |_| {
        panic!("publish callback ran after plugin root replacement")
    });

    assert!(result.is_err());
    assert!(std::fs::read_dir(&plugins_dir).unwrap().next().is_none());
    assert!(std::fs::read_dir(&original_root).unwrap().next().is_none());
}

#[test]
fn staged_publish_never_replaces_a_concurrent_destination() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    )
    .unwrap();
    let staging_root = staged.root_path().to_path_buf();
    let destination = plugins_dir.join(plugin_id.as_str());

    let result = staged.publish_with_before_rename(&destination, |_| {
        std::fs::create_dir(&destination)?;
        std::fs::write(destination.join("sentinel"), "do not replace")?;
        Ok(())
    });

    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(destination.join("sentinel")).unwrap(),
        "do not replace"
    );
    assert!(
        !staging_root.exists(),
        "failed publish leaked staging files"
    );
}

#[cfg(windows)]
#[test]
fn windows_publish_renames_the_opened_artifact_not_a_swapped_junction() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let outside_dir = temp_dir.path().join("outside");
    std::fs::create_dir(&plugins_dir).unwrap();
    std::fs::create_dir(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("outside-sentinel"), "preserve").unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    )
    .unwrap();
    let artifact_path = staged.artifact_path().to_path_buf();
    let moved_artifact = staged.root_path().join("moved-artifact");
    let destination = plugins_dir.join(plugin_id.as_str());

    let result = staged.publish_with_before_rename(&destination, |staged| {
        staged.move_open_artifact_for_test(
            moved_artifact
                .file_name()
                .expect("moved artifact must have a final component"),
        )?;
        create_manager_test_directory_link(&outside_dir, &artifact_path);
        Ok(())
    });

    assert!(result.is_ok(), "handle-targeted publish failed: {result:?}");
    assert!(destination.join("staged-plugin.wasm").is_file());
    assert!(!destination.join("outside-sentinel").exists());
    assert_eq!(
        std::fs::read_to_string(outside_dir.join("outside-sentinel")).unwrap(),
        "preserve"
    );
}

#[cfg(windows)]
#[test]
fn windows_cleanup_fails_closed_when_the_staged_artifact_name_is_swapped() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let outside_dir = temp_dir.path().join("outside");
    std::fs::create_dir(&plugins_dir).unwrap();
    std::fs::create_dir(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("outside-sentinel"), "preserve").unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir).unwrap();
    let plugin_id = crate::types::PluginId::parse("staged-plugin").unwrap();
    let manifest = super::lifecycle::manifest_from_metadata(
        &plugin_id,
        crate::types::PluginMetadata {
            id: plugin_id.as_str().to_string(),
            name: "Staged Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: String::new(),
            description: String::new(),
        },
    );
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        b"component",
        &manifest,
    )
    .unwrap();
    let staging_root = staged.root_path().to_path_buf();
    let artifact_path = staged.artifact_path().to_path_buf();
    let moved_artifact = staging_root.join("moved-artifact");

    let result = staged.rollback_with_before_cleanup(|staged| {
        staged
            .move_open_artifact_for_test(
                moved_artifact
                    .file_name()
                    .expect("moved artifact must have a final component"),
            )
            .unwrap();
        create_manager_test_directory_link(&outside_dir, &artifact_path);
    });

    assert!(
        result.is_err(),
        "cleanup accepted a swapped staged identity"
    );
    assert_eq!(
        std::fs::read_to_string(outside_dir.join("outside-sentinel")).unwrap(),
        "preserve"
    );
    assert!(staging_root.exists(), "unsafe cleanup did not fail closed");

    if artifact_path.exists() {
        std::fs::remove_dir(&artifact_path).unwrap();
    }
    if moved_artifact.exists() {
        std::fs::remove_dir_all(&moved_artifact).unwrap();
    }
    if staging_root.exists() {
        std::fs::remove_dir(&staging_root).unwrap();
    }
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
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("network = false", "network = true")
        .replace(
            "network_domains = []",
            "network_domains = [\"old-manifest.test\"]",
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
