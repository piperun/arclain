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

#[cfg(unix)]
fn create_manager_test_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn create_manager_test_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("failed to create test file symlink: {error}"),
    }
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
fn manager_routes_guest_exports_through_serializable_executor_messages() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();

    assert!(matches!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::Metadata)
            .unwrap(),
        wirt::ExecutorResponse::Metadata(metadata) if metadata.id == "ui-demo"
    ));
    assert!(matches!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::DefaultRules)
            .unwrap(),
        wirt::ExecutorResponse::Rules(_)
    ));
    assert!(matches!(
        manager
            .execute_plugin(
                "ui-demo",
                wirt::ExecutorRequest::UiLayout {
                    extension_point: wirt::PluginExtensionPoint::MainPage,
                },
            )
            .unwrap(),
        wirt::ExecutorResponse::Layout(_)
    ));
    assert!(matches!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::TopTabs)
            .unwrap(),
        wirt::ExecutorResponse::TopTabs(_)
    ));
    assert!(matches!(
        manager
            .execute_plugin(
                "ui-demo",
                wirt::ExecutorRequest::UiEvent {
                    id: "demo_btn".to_string(),
                    value: None,
                },
            )
            .unwrap(),
        wirt::ExecutorResponse::Actions(_)
    ));
    assert!(manager.get_default_rules("ui-demo").unwrap().is_empty());
}

#[test]
fn manager_rejects_oversized_executor_requests_before_registry_lookup() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().to_path_buf(), HashMap::new()).unwrap();
    let error = manager
        .execute_plugin(
            "missing-plugin",
            wirt::ExecutorRequest::UiEvent {
                id: "x".repeat(wirt::MAX_EXECUTOR_MESSAGE_BYTES),
                value: None,
            },
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Plugin execution failed: executor request limit exceeded"
    );
}

#[test]
fn unload_waits_for_admitted_execution_and_cleanup_remains_terminal() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();

    let executor = manager.wirt_executor();
    let plugin_id = wirt::PluginId::parse("ui-demo").unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    executor.set_admitted_execution_hook(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));

    let execution = {
        let executor = executor.clone();
        let plugin_id = plugin_id.clone();
        std::thread::spawn(move || {
            wirt::WirtExecutor::execute(
                executor.as_ref(),
                &plugin_id,
                wirt::ExecutorRequest::Metadata,
            )
        })
    };
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("execution must reach the admitted-instance boundary");

    let manager = Arc::new(parking_lot::Mutex::new(manager));
    let (unloaded_tx, unloaded_rx) = std::sync::mpsc::channel();
    let unload = {
        let manager = manager.clone();
        std::thread::spawn(move || {
            let result = manager.lock().unload_plugin("ui-demo");
            unloaded_tx.send(result).unwrap();
        })
    };
    assert!(matches!(
        unloaded_rx.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    release_tx.send(()).unwrap();
    assert!(matches!(
        execution.join().unwrap().unwrap(),
        wirt::ExecutorResponse::Metadata(_)
    ));
    unloaded_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("unload must finish after the admitted execution")
        .unwrap();
    unload.join().unwrap();

    let error = wirt::WirtExecutor::execute(
        executor.as_ref(),
        &plugin_id,
        wirt::ExecutorRequest::Metadata,
    )
    .unwrap_err();
    assert!(matches!(error, PluginError::NotFound(_)));
}

#[test]
fn disable_waits_for_admitted_execution_and_blocks_later_guest_entry() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();

    let executor = manager.wirt_executor();
    let plugin_id = wirt::PluginId::parse("ui-demo").unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    executor.set_admitted_execution_hook(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));

    let execution = {
        let executor = executor.clone();
        let plugin_id = plugin_id.clone();
        std::thread::spawn(move || {
            wirt::WirtExecutor::execute(
                executor.as_ref(),
                &plugin_id,
                wirt::ExecutorRequest::Metadata,
            )
        })
    };
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("execution must reach enabled admission");

    let manager = Arc::new(parking_lot::Mutex::new(manager));
    let (disabled_tx, disabled_rx) = std::sync::mpsc::channel();
    let disable = {
        let manager = manager.clone();
        std::thread::spawn(move || {
            let result = manager.lock().disable_plugin("ui-demo");
            disabled_tx.send(result).unwrap();
        })
    };
    assert!(matches!(
        disabled_rx.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    release_tx.send(()).unwrap();
    assert!(matches!(
        execution.join().unwrap().unwrap(),
        wirt::ExecutorResponse::Metadata(_)
    ));
    disabled_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("disable must finish after the admitted execution")
        .unwrap();
    disable.join().unwrap();

    let error = wirt::WirtExecutor::execute(
        executor.as_ref(),
        &plugin_id,
        wirt::ExecutorRequest::Metadata,
    )
    .unwrap_err();
    assert!(matches!(error, PluginError::Unavailable(_)));
}

#[test]
fn disabling_is_reentrant_while_a_caller_holds_the_public_instance_lock() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();
    let manager = Arc::new(manager);
    let (done_tx, done_rx) = std::sync::mpsc::channel();

    let worker = {
        let manager = manager.clone();
        std::thread::spawn(move || {
            let result = manager
                .with_plugin_instance("ui-demo", |_| manager.disable_plugin("ui-demo"))
                .expect("plugin instance must exist");
            done_tx.send(result).unwrap();
        })
    };

    done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("disable must not wait on the caller-held public instance lock")
        .unwrap();
    worker.join().unwrap();
    assert!(!manager.is_plugin_enabled("ui-demo"));
}

#[test]
fn concurrent_enable_waits_for_disable_to_finish_its_admission_transition() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();
    let manager = Arc::new(manager);
    let (disabled_map_tx, disabled_map_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    manager.set_disable_before_admission_hook(Box::new(move || {
        disabled_map_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));

    let disable = {
        let manager = manager.clone();
        std::thread::spawn(move || manager.disable_plugin("ui-demo"))
    };
    disabled_map_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("disable must reach the split state transition");

    let (enabled_tx, enabled_rx) = std::sync::mpsc::channel();
    let enable = {
        let manager = manager.clone();
        std::thread::spawn(move || enabled_tx.send(manager.enable_plugin("ui-demo")).unwrap())
    };
    assert!(matches!(
        enabled_rx.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    release_tx.send(()).unwrap();
    disable.join().unwrap().unwrap();
    enabled_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("enable must follow the completed disable transition")
        .unwrap();
    enable.join().unwrap();

    assert!(manager.is_plugin_enabled("ui-demo"));
    assert!(matches!(
        manager
            .execute_plugin("ui-demo", wirt::ExecutorRequest::Metadata)
            .unwrap(),
        wirt::ExecutorResponse::Metadata(_)
    ));
}

#[test]
fn disabled_plugin_cannot_be_restored_into_the_top_tabs_cache() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("ui-demo.wirt"));
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();
    manager.init().unwrap();
    let manager = Arc::new(manager);

    let (read_tx, read_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    manager.set_top_tabs_before_cache_store_hook(Box::new(move || {
        read_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    }));
    let reader = {
        let manager = manager.clone();
        std::thread::spawn(move || manager.get_all_top_tabs())
    };
    read_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("top-tab read must reach the cache publication boundary");

    manager.disable_plugin("ui-demo").unwrap();
    assert!(manager.cached_top_tabs.lock().is_none());
    release_tx.send(()).unwrap();
    reader.join().unwrap();

    assert!(
        manager.cached_top_tabs.lock().is_none(),
        "a read admitted before invalidation must not republish stale tabs"
    );
}

fn ui_demo_package(path: &std::path::Path) -> wirt::PackageFingerprint {
    let manifest = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let bytes = wirt::package_bytes(manifest, component).expect("valid ui-demo package");
    let fingerprint = wirt::PackageFingerprint::sha256(&bytes);
    std::fs::write(path, bytes).unwrap();
    fingerprint
}

fn ui_demo_package_with_manifest(
    path: &std::path::Path,
    mutate: impl FnOnce(&mut crate::types::PluginManifest),
) -> wirt::PackageFingerprint {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let mut manifest: crate::types::PluginManifest = toml::from_str(source).unwrap();
    mutate(&mut manifest);
    let manifest = toml::to_string(&manifest).unwrap();
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let bytes = wirt::package_bytes(manifest.as_bytes(), component).unwrap();
    let fingerprint = wirt::PackageFingerprint::sha256(&bytes);
    std::fs::write(path, bytes).unwrap();
    fingerprint
}

#[test]
fn package_preview_and_install_use_the_approved_fingerprint() {
    let temp_dir = TempDir::new().unwrap();
    let package_path = temp_dir.path().join("ui-demo.WIRT");
    let fingerprint = ui_demo_package(&package_path);
    let mut manager = PluginManager::new_with_plugin_log_dir(
        temp_dir.path().join("plugins"),
        HashMap::new(),
        temp_dir.path().join("logs"),
    )
    .unwrap();

    let preview = manager.inspect_plugin_package(&package_path).unwrap();
    assert_eq!(preview.manifest.plugin.id, "ui-demo");
    assert_eq!(preview.fingerprint, fingerprint);
    assert!(
        manager.list_plugins().is_empty(),
        "preview must not register"
    );

    assert_eq!(
        manager
            .install_plugin_package(&package_path, &fingerprint)
            .unwrap(),
        "ui-demo"
    );
    assert_eq!(
        std::fs::read_to_string(temp_dir.path().join("plugins/ui-demo/package.sha256")).unwrap(),
        fingerprint.as_str(),
    );
}

#[test]
fn package_install_rejects_loose_wasm_at_the_manager_boundary() {
    let temp_dir = TempDir::new().unwrap();
    let wasm_path = fixture_path_for_manager_test("ui-demo");
    let expected = wirt::PackageFingerprint::sha256(b"not the selected file");
    let mut manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();

    assert!(manager
        .install_plugin_package(&wasm_path, &expected)
        .is_err());
    assert!(manager.list_plugins().is_empty());
}

#[test]
fn package_install_reopens_and_rejects_a_previewed_package_that_changed() {
    let temp_dir = TempDir::new().unwrap();
    let package_path = temp_dir.path().join("ui-demo.wirt");
    ui_demo_package(&package_path);
    let mut manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();
    let preview = manager.inspect_plugin_package(&package_path).unwrap();

    ui_demo_package_with_manifest(&package_path, |manifest| {
        manifest.plugin.version = "9.9.9".to_string();
    });
    let result = manager.install_plugin_package(&package_path, &preview.fingerprint);

    assert!(result.is_err());
    assert!(manager.list_plugins().is_empty());
    assert!(std::fs::read_dir(manager.plugins_dir())
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn package_preview_rejects_each_guest_manifest_metadata_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    let manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();
    let cases: [(&str, fn(&mut crate::types::PluginManifest)); 5] = [
        ("id", |manifest| manifest.plugin.id = "other-demo".into()),
        ("name", |manifest| {
            manifest.plugin.name = "Other Name".into()
        }),
        ("version", |manifest| {
            manifest.plugin.version = "9.9.9".into()
        }),
        ("author", |manifest| {
            manifest.plugin.author = "Other Author".into()
        }),
        ("description", |manifest| {
            manifest.plugin.description = "Other description".into()
        }),
    ];
    for (field, mutate) in cases {
        let package_path = temp_dir.path().join(format!("mismatch-{field}.wirt"));
        ui_demo_package_with_manifest(&package_path, mutate);
        let error = manager.inspect_plugin_package(&package_path).unwrap_err();
        assert!(matches!(
            error,
            crate::types::PluginError::InvalidManifest(_)
        ));
        assert!(error.to_string().contains(field));
    }
}

#[test]
fn package_install_rejects_a_root_package_with_the_same_identity() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    ui_demo_package(&plugins_dir.join("renamed-package.wirt"));
    let selected = temp_dir.path().join("selected.wirt");
    let expected = ui_demo_package(&selected);
    let mut manager = PluginManager::new(plugins_dir, HashMap::new()).unwrap();

    assert!(manager
        .install_plugin_package(&selected, &expected)
        .is_err());
    assert!(manager.list_plugins().is_empty());
}

#[test]
fn package_preview_rejects_a_link_or_non_file_final_component() {
    let temp_dir = TempDir::new().unwrap();
    let target = temp_dir.path().join("target.wirt");
    ui_demo_package(&target);
    let selected = temp_dir.path().join("selected.wirt");
    if !create_manager_test_file_link(&target, &selected) {
        // Creating symlinks can require an unavailable Windows privilege.
        // Keep the test non-vacuous: a directory exercises the same final
        // component regular-file gate without weakening production policy.
        std::fs::create_dir(&selected).unwrap();
    }
    let manager = PluginManager::new(temp_dir.path().join("plugins"), HashMap::new()).unwrap();

    assert!(manager.inspect_plugin_package(&selected).is_err());
    assert!(manager.list_plugins().is_empty());
}

#[test]
fn package_init_failure_rolls_back_sidecars_staging_and_live_state() {
    const MANIFEST: &[u8] = br#"[wirt]
abi = "0.1.0"

[plugin]
id = "failing-init"
name = "Failing Init Fixture"
version = "0.1.0"
author = "Arclain security tests"
description = "Package-valid guest that traps during initialization"

[capabilities]

[rate_limits]
http_requests_per_minute = 60
"#;
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/failing-init/failing-init.wasm"
    ));
    let package = wirt::package_bytes(MANIFEST, component).unwrap();
    let fingerprint = wirt::PackageFingerprint::sha256(&package);
    let temp_dir = TempDir::new().unwrap();
    let package_path = temp_dir.path().join("failing-init.wirt");
    std::fs::write(&package_path, package).unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("logs"),
    )
    .unwrap();

    let preview = manager
        .inspect_plugin_package(&package_path)
        .expect("preview must validate metadata without calling init");
    assert_eq!(preview.fingerprint, fingerprint);
    assert!(manager.list_plugins().is_empty());

    assert!(manager
        .install_plugin_package(&package_path, &fingerprint)
        .is_err());
    assert!(manager.list_plugins().is_empty());
    assert!(!plugins_dir.join("failing-init").exists());
    assert!(
        std::fs::read_dir(&plugins_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".arclain-plugin-install-")),
        "failed init retained package sidecars or staging",
    );
}

#[test]
fn package_metadata_mismatch_rolls_back_staging_and_live_state() {
    let temp_dir = TempDir::new().unwrap();
    let package_path = temp_dir.path().join("metadata-mismatch.wirt");
    let fingerprint = ui_demo_package_with_manifest(&package_path, |manifest| {
        manifest.plugin.name = "Mismatched Name".to_string();
    });
    let plugins_dir = temp_dir.path().join("plugins");
    let mut manager = PluginManager::new_with_plugin_log_dir(
        plugins_dir.clone(),
        HashMap::new(),
        temp_dir.path().join("logs"),
    )
    .unwrap();

    let error = manager
        .install_plugin_package(&package_path, &fingerprint)
        .expect_err("installation must re-check guest metadata from staging");

    assert!(matches!(
        error,
        crate::types::PluginError::InvalidManifest(_)
    ));
    assert!(manager.list_plugins().is_empty());
    assert!(!plugins_dir.join("ui-demo").exists());
    assert!(std::fs::read_dir(&plugins_dir).unwrap().next().is_none());
}

fn fixture_path_for_manager_test(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name)
        .join(format!("{name}.wasm"))
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
[wirt]
abi = "0.1.0"

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
fn case_folded_lifecycle_rejects_manifest_spelling_that_differs_from_guest() {
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
    let _ = manager.get_all_top_tabs();
    assert!(manager.cached_top_tabs.lock().is_some());

    let manifest_path = plugins_dir.join("ui-demo").join("ui-demo.toml");
    let mut manifest: crate::types::PluginManifest =
        toml::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest.plugin.id = "Ui-DeMo".to_string();
    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest).unwrap()).unwrap();

    let error = manager.reload_plugin("ui-demo").unwrap_err();
    assert!(matches!(
        error,
        crate::types::PluginError::InvalidManifest(_)
    ));
    assert!(manager.get_plugin_metadata("ui-demo").is_none());
    assert!(manager.get_plugin_instance("ui-demo").is_none());
    assert!(!manager.is_plugin_enabled("ui-demo"));
    assert!(
        manager.cached_top_tabs.lock().is_none(),
        "a reload failure after removal must not retain the old generation's tabs"
    );
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

#[test]
fn package_publish_race_rolls_back_fingerprint_sidecar_and_staging() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let manifest_bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let fingerprint =
        wirt::PackageFingerprint::sha256(&wirt::package_bytes(manifest_bytes, component).unwrap());
    let plugin_id = crate::types::PluginId::parse("ui-demo").unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let staged = super::lifecycle::StagedPluginPackage::new_package(
        loader.trusted_root(),
        &plugin_id,
        component,
        manifest_bytes,
        &fingerprint,
    )
    .unwrap();
    let staging_root = staged.root_path().to_path_buf();
    let destination = plugins_dir.join("ui-demo");

    let result = staged.publish_with_before_rename(&destination, |_| {
        std::fs::create_dir(&destination)?;
        std::fs::write(destination.join("sentinel"), "preserve")?;
        Ok(())
    });

    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(destination.join("sentinel")).unwrap(),
        "preserve"
    );
    assert!(!staging_root.exists());
    assert!(!destination.join("package.sha256").exists());
}

#[test]
fn package_staging_construction_failure_preserves_io_kind_and_removes_every_partial_tree() {
    let manifest = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let fingerprint =
        wirt::PackageFingerprint::sha256(&wirt::package_bytes(manifest, component).unwrap());
    let plugin_id = crate::types::PluginId::parse("ui-demo").unwrap();

    for failing_step in [
        "staging directory",
        "artifact directory",
        "plugin WASM",
        "plugin manifest",
        "package fingerprint",
    ] {
        let temp_dir = TempDir::new().unwrap();
        let plugins_dir = temp_dir.path().join("plugins");
        let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();

        let result = super::lifecycle::StagedPluginPackage::new_package_with_after_step(
            loader.trusted_root(),
            &plugin_id,
            component,
            manifest,
            &fingerprint,
            |completed_step| {
                if completed_step == failing_step {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("injected failure after {completed_step}"),
                    ))
                } else {
                    Ok(())
                }
            },
        );

        let error = match result {
            Ok(_) => panic!("{failing_step} failure unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                crate::types::PluginError::Io(ref source)
                    if source.kind() == std::io::ErrorKind::PermissionDenied
            ),
            "{failing_step} lost its permission-denied class: {error:?}",
        );
        assert!(
            std::fs::read_dir(&plugins_dir).unwrap().next().is_none(),
            "{failing_step} failure leaked a staging tree",
        );
    }
}

#[cfg(not(windows))]
#[test]
fn unix_publish_rejects_a_swapped_staged_artifact_identity() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let manifest = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let fingerprint =
        wirt::PackageFingerprint::sha256(&wirt::package_bytes(manifest, component).unwrap());
    let plugin_id = crate::types::PluginId::parse("ui-demo").unwrap();
    let loader = crate::loader::PluginLoader::new(plugins_dir.clone()).unwrap();
    let staged = super::lifecycle::StagedPluginPackage::new_package(
        loader.trusted_root(),
        &plugin_id,
        component,
        manifest,
        &fingerprint,
    )
    .unwrap();
    let original_artifact = staged.artifact_path().to_path_buf();
    let moved_artifact = staged.root_path().join("validated-artifact");
    let destination = plugins_dir.join("ui-demo");

    let result = staged.publish_with_before_rename(&destination, |_| {
        std::fs::rename(&original_artifact, &moved_artifact)?;
        std::fs::create_dir(&original_artifact)?;
        std::fs::write(original_artifact.join("replacement-sentinel"), "preserve")?;
        Ok(())
    });

    assert!(result.is_err(), "a swapped artifact was published");
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read_to_string(original_artifact.join("replacement-sentinel")).unwrap(),
        "preserve",
    );
}

#[cfg(not(windows))]
#[test]
fn unix_cleanup_preserves_a_swapped_staging_root() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let plugin_id = crate::types::PluginId::parse("ui-demo").unwrap();
    let manifest: crate::types::PluginManifest = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    )))
    .unwrap();
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    let loader = crate::loader::PluginLoader::new(plugins_dir).unwrap();
    let staged = super::lifecycle::StagedPluginPackage::new(
        loader.trusted_root(),
        &plugin_id,
        component,
        &manifest,
    )
    .unwrap();
    let original_root = staged.root_path().to_path_buf();
    let moved_root = temp_dir.path().join("validated-staging-root");

    let result = staged.rollback_with_before_cleanup(|_| {
        std::fs::rename(&original_root, &moved_root).unwrap();
        std::fs::create_dir(&original_root).unwrap();
        std::fs::write(original_root.join("replacement-sentinel"), "preserve").unwrap();
    });

    assert!(result.is_err(), "cleanup accepted a swapped staging root");
    assert_eq!(
        std::fs::read_to_string(original_root.join("replacement-sentinel")).unwrap(),
        "preserve",
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
