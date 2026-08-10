use super::*;
use tempfile::TempDir;

fn valid_manifest(id: &str) -> PluginManifest {
    let mut manifest = valid_manifest_with_abi(crate::WIRT_ABI_VERSION);
    manifest.plugin.id = id.to_string();
    manifest
}

fn valid_manifest_with_abi(abi: &str) -> PluginManifest {
    PluginManifest {
        wirt: crate::WirtConfig {
            abi: abi.to_string(),
        },
        plugin: crate::PluginInfoConfig {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test plugin".to_string(),
        },
        capabilities: crate::CapabilitiesConfig::default(),
        rate_limits: Default::default(),
    }
}

fn manifest_toml_without_wirt_table() -> &'static str {
    r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
author = "Test Author"
description = "A test plugin"

[capabilities]
"#
}

fn write_plugin_pair(directory: &std::path::Path, file_id: &str, manifest_id: &str) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join(format!("{file_id}.toml")),
        toml::to_string(&valid_manifest(manifest_id)).unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join(format!("{file_id}.wasm")), b"component").unwrap();
}

fn write_ui_demo_sidecars(directory: &std::path::Path, with_fingerprint: bool) {
    std::fs::create_dir_all(directory).unwrap();
    let manifest = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    std::fs::write(directory.join("ui-demo.toml"), manifest).unwrap();
    std::fs::write(directory.join("ui-demo.wasm"), component).unwrap();
    if with_fingerprint {
        let package = crate::package_bytes(manifest, component).unwrap();
        std::fs::write(
            directory.join("package.sha256"),
            crate::PackageFingerprint::sha256(&package).as_str(),
        )
        .unwrap();
    }
}

#[cfg(unix)]
fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("failed to create test file symlink: {error}"),
    }
}

#[cfg(unix)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) {
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

#[test]
fn test_plugin_loader_creation() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf());
    assert!(loader.is_ok());
}

#[test]
fn discovery_accepts_a_root_wirt_package_and_legacy_sidecars() {
    let temp_dir = TempDir::new().unwrap();
    let manifest = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    std::fs::write(
        temp_dir.path().join("ui-demo.WIRT"),
        crate::package_bytes(manifest, component).unwrap(),
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("loose.wasm"), component).unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
    let discovered = loader.discover_plugins().unwrap();
    assert_eq!(discovered.len(), 1);
    assert!(matches!(
        discovered[0].artifact,
        PluginArtifact::Package { .. }
    ));
    loader
        .load_plugin(&discovered[0])
        .expect("a discovered root package must compile and load");

    let legacy = TempDir::new().unwrap();
    write_ui_demo_sidecars(&legacy.path().join("ui-demo"), false);
    let loader = PluginLoader::new(legacy.path().to_path_buf()).unwrap();
    let discovered = loader.discover_plugins().unwrap();
    assert_eq!(discovered.len(), 1);
    assert!(matches!(
        discovered[0].artifact,
        PluginArtifact::Sidecars { .. }
    ));
    loader
        .load_plugin(&discovered[0])
        .expect("discovered legacy sidecars must compile and load");
}

#[test]
fn discovery_rejects_case_folded_identity_duplicates_across_artifact_kinds() {
    let temp_dir = TempDir::new().unwrap();
    write_ui_demo_sidecars(&temp_dir.path().join("ui-demo"), false);

    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.toml"
    ));
    let mut manifest: crate::PluginManifest = toml::from_str(source).unwrap();
    manifest.plugin.id = "UI-DEMO".to_string();
    let component = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    std::fs::write(
        temp_dir.path().join("renamed.wirt"),
        crate::package_bytes(toml::to_string(&manifest).unwrap().as_bytes(), component).unwrap(),
    )
    .unwrap();

    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
    assert!(loader.discover_plugins().is_err());
}

#[test]
fn package_fingerprint_metadata_error_preserves_io_kind() {
    let error = package_fingerprint_metadata_error(std::io::Error::from(
        std::io::ErrorKind::PermissionDenied,
    ));

    assert!(matches!(
        error,
        PluginError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied
    ));
}

#[test]
fn package_fingerprint_sidecar_is_verified_on_discovery_and_load() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("ui-demo");
    write_ui_demo_sidecars(&plugin_dir, true);
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
    let discovered = loader.discover_plugins().unwrap();
    assert_eq!(discovered.len(), 1);

    std::fs::write(plugin_dir.join("package.sha256"), "0".repeat(64)).unwrap();
    assert!(loader.load_plugin(&discovered[0]).is_err());

    let malformed = TempDir::new().unwrap();
    let plugin_dir = malformed.path().join("ui-demo");
    write_ui_demo_sidecars(&plugin_dir, true);
    std::fs::write(plugin_dir.join("package.sha256"), "not-a-fingerprint").unwrap();
    let loader = PluginLoader::new(malformed.path().to_path_buf()).unwrap();
    assert!(loader.discover_plugins().unwrap().is_empty());
}

#[test]
fn manifest_requires_the_current_wirt_abi() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let manifest = valid_manifest_with_abi("0.1.0");
    loader.validate_manifest(&manifest).unwrap();

    let missing = manifest_toml_without_wirt_table();
    assert!(toml::from_str::<PluginManifest>(missing).is_err());

    let error = loader
        .validate_manifest(&valid_manifest_with_abi("0.2.0"))
        .unwrap_err();
    assert!(matches!(error, PluginError::Unsupported(_)));
    assert!(error.to_string().contains("unsupported Wirt ABI"));
}

#[test]
fn test_manifest_validation() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let manifest = PluginManifest {
        wirt: crate::WirtConfig {
            abi: crate::WIRT_ABI_VERSION.to_string(),
        },
        plugin: crate::PluginInfoConfig {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test plugin".to_string(),
        },
        capabilities: crate::CapabilitiesConfig {
            network: false,
            network_domains: vec![],
            archive_metadata_read: true,
            archive_metadata_write: false,
            archive_modify: false,
            file_read: false,
            file_write: false,
        },
        rate_limits: Default::default(),
    };

    assert!(loader.validate_manifest(&manifest).is_ok());
}

#[test]
fn test_invalid_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let manifest = PluginManifest {
        wirt: crate::WirtConfig {
            abi: crate::WIRT_ABI_VERSION.to_string(),
        },
        plugin: crate::PluginInfoConfig {
            id: "".to_string(), // Empty ID
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            author: "Test Author".to_string(),
            description: "A test plugin".to_string(),
        },
        capabilities: Default::default(),
        rate_limits: Default::default(),
    };

    assert!(loader.validate_manifest(&manifest).is_err());
}

#[test]
fn plugin_id_is_one_portable_non_reserved_component() {
    for invalid in [
        "", ".", "..", "a/b", "a\\b", "C:", "CON", "con.txt", "NUL", "COM1", "LPT9", "name.",
        "name ", "café", "рlugin",
    ] {
        assert!(
            crate::PluginId::parse(invalid).is_err(),
            "accepted unsafe plugin id {invalid:?}",
        );
    }

    for valid in ["dlsite-metadata", "ui_demo", "Plugin2"] {
        assert_eq!(crate::PluginId::parse(valid).unwrap().as_str(), valid);
    }
}

#[test]
fn plugin_id_rejects_more_than_64_bytes() {
    assert!(crate::PluginId::parse("a".repeat(64)).is_ok());
    assert!(crate::PluginId::parse("a".repeat(65)).is_err());
}

#[test]
fn manifest_rejects_oversized_retained_fields() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let mut manifest = valid_manifest("bounded-fields");
    manifest.plugin.name = "n".repeat(129);
    assert!(loader.validate_manifest(&manifest).is_err());

    let mut manifest = valid_manifest("bounded-fields");
    manifest.plugin.version = "v".repeat(65);
    assert!(loader.validate_manifest(&manifest).is_err());

    let mut manifest = valid_manifest("bounded-fields");
    manifest.plugin.author = "a".repeat(257);
    assert!(loader.validate_manifest(&manifest).is_err());

    let mut manifest = valid_manifest("bounded-fields");
    manifest.plugin.description = "d".repeat(16 * 1024 + 1);
    assert!(loader.validate_manifest(&manifest).is_err());
}

#[test]
fn manifest_requires_canonical_bounded_unique_hostnames() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    for invalid in [
        "HTTPS://example.com",
        "Example.com",
        "example.com.",
        "example.com:443",
        "*.example.com",
        "-bad.example",
        "bad-.example",
        "bad..example",
        "café.example",
    ] {
        let mut manifest = valid_manifest("domain-policy");
        manifest.capabilities.network = true;
        manifest.capabilities.network_domains = vec![invalid.to_string()];
        assert!(
            loader.validate_manifest(&manifest).is_err(),
            "accepted non-canonical hostname {invalid:?}"
        );
    }

    let mut duplicate = valid_manifest("domain-policy");
    duplicate.capabilities.network = true;
    duplicate.capabilities.network_domains =
        vec!["api.example.com".to_string(), "api.example.com".to_string()];
    assert!(loader.validate_manifest(&duplicate).is_err());

    let mut too_many = valid_manifest("domain-policy");
    too_many.capabilities.network = true;
    too_many.capabilities.network_domains = (0..=64)
        .map(|index| format!("host-{index}.example.com"))
        .collect();
    assert!(loader.validate_manifest(&too_many).is_err());

    let mut valid = valid_manifest("domain-policy");
    valid.capabilities.network = true;
    valid.capabilities.network_domains = vec![
        "api.example.com".to_string(),
        "xn--bcher-kva.example".to_string(),
        "127.0.0.1".to_string(),
    ];
    assert!(loader.validate_manifest(&valid).is_ok());
}

#[test]
fn manifest_rejects_excessive_network_rate() {
    let temp_dir = TempDir::new().unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();
    let mut manifest = valid_manifest("rate-policy");

    manifest.rate_limits.http_requests_per_minute = 601;

    assert!(loader.validate_manifest(&manifest).is_err());
}

#[test]
fn discovery_rejects_manifest_larger_than_64_kib() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("oversized-manifest");
    std::fs::create_dir(&plugin_dir).unwrap();
    let mut manifest = valid_manifest("oversized-manifest");
    manifest.plugin.description = "d".repeat(65 * 1024);
    std::fs::write(
        plugin_dir.join("oversized-manifest.toml"),
        toml::to_string(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("oversized-manifest.wasm"),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        )),
    )
    .unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_wasm_larger_than_64_mib_without_reading_it() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("oversized-wasm");
    std::fs::create_dir(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("oversized-wasm.toml"),
        toml::to_string(&valid_manifest("oversized-wasm")).unwrap(),
    )
    .unwrap();
    let file = std::fs::File::create(plugin_dir.join("oversized-wasm.wasm")).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_folder_name_manifest_id_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("folder-name");
    std::fs::create_dir(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("folder-name.toml"),
        toml::to_string(&valid_manifest("different-id")).unwrap(),
    )
    .unwrap();
    std::fs::write(plugin_dir.join("folder-name.wasm"), b"component").unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_flat_file_name_manifest_id_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(
        temp_dir.path().join("file-name.toml"),
        toml::to_string(&valid_manifest("different-id")).unwrap(),
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("file-name.wasm"), b"component").unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_duplicate_ids_across_layouts() {
    let temp_dir = TempDir::new().unwrap();
    let plugin_dir = temp_dir.path().join("duplicate-id");
    std::fs::create_dir(&plugin_dir).unwrap();
    let manifest = toml::to_string(&valid_manifest("duplicate-id")).unwrap();
    std::fs::write(plugin_dir.join("duplicate-id.toml"), &manifest).unwrap();
    std::fs::write(plugin_dir.join("duplicate-id.wasm"), b"component").unwrap();
    std::fs::write(temp_dir.path().join("duplicate-id.toml"), manifest).unwrap();
    std::fs::write(temp_dir.path().join("duplicate-id.wasm"), b"component").unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let error = loader.discover_plugins().unwrap_err();

    assert!(matches!(error, PluginError::InvalidManifest(_)));
    assert!(error.to_string().contains("duplicate-id"));
}

#[test]
fn discovery_rejects_case_folded_duplicate_ids_across_layouts() {
    let temp_dir = TempDir::new().unwrap();
    write_plugin_pair(&temp_dir.path().join("Plugin2"), "Plugin2", "Plugin2");
    std::fs::write(
        temp_dir.path().join("plugin2.toml"),
        toml::to_string(&valid_manifest("plugin2")).unwrap(),
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("plugin2.wasm"), b"component").unwrap();
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let error = loader.discover_plugins().unwrap_err();

    assert!(matches!(error, PluginError::InvalidManifest(_)));
    assert!(error.to_string().to_ascii_lowercase().contains("plugin2"));
}

#[test]
fn discovery_compares_manifest_and_file_ids_by_portable_identity_key() {
    let temp_dir = TempDir::new().unwrap();
    write_plugin_pair(&temp_dir.path().join("Plugin2"), "Plugin2", "plugin2");
    let loader = PluginLoader::new(temp_dir.path().to_path_buf()).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].manifest.plugin.id, "plugin2");
}

#[test]
fn direct_discovery_rejects_a_manifest_outside_the_configured_root() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let outside_dir = temp_dir.path().join("outside");
    write_plugin_pair(&outside_dir, "outside-plugin", "outside-plugin");
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let result = loader.discover_plugin_from_folder(&outside_dir.join("outside-plugin.toml"));

    assert!(result.is_err());
}

#[test]
fn discovery_rejects_a_linked_plugin_folder_or_windows_junction() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let outside_dir = temp_dir.path().join("outside-folder");
    write_plugin_pair(&outside_dir, "linked-plugin", "linked-plugin");
    create_directory_link(&outside_dir, &plugins_dir.join("linked-plugin"));
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_a_linked_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_dir = plugins_dir.join("linked-manifest");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let outside_manifest = temp_dir.path().join("outside.toml");
    std::fs::write(
        &outside_manifest,
        toml::to_string(&valid_manifest("linked-manifest")).unwrap(),
    )
    .unwrap();
    if !create_file_link(&outside_manifest, &plugin_dir.join("linked-manifest.toml")) {
        return;
    }
    std::fs::write(plugin_dir.join("linked-manifest.wasm"), b"component").unwrap();
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn discovery_rejects_a_linked_wasm() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_dir = plugins_dir.join("linked-wasm");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("linked-wasm.toml"),
        toml::to_string(&valid_manifest("linked-wasm")).unwrap(),
    )
    .unwrap();
    let outside_wasm = temp_dir.path().join("outside.wasm");
    std::fs::write(&outside_wasm, b"component").unwrap();
    if !create_file_link(&outside_wasm, &plugin_dir.join("linked-wasm.wasm")) {
        return;
    }
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let discovered = loader.discover_plugins().unwrap();

    assert!(discovered.is_empty());
}

#[test]
fn bounded_read_does_not_follow_a_parent_swapped_after_validation() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_dir = plugins_dir.join("race-plugin");
    let moved_plugin_dir = temp_dir.path().join("validated-plugin");
    let outside_dir = temp_dir.path().join("outside-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::create_dir(&outside_dir).unwrap();
    let manifest_path = plugin_dir.join("race-plugin.toml");
    std::fs::write(&manifest_path, b"trusted").unwrap();
    std::fs::write(outside_dir.join("race-plugin.toml"), b"outside").unwrap();
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let result = loader.read_plugin_file_bounded_with_hook(
        &manifest_path,
        MAX_PLUGIN_MANIFEST_BYTES,
        "plugin manifest",
        || {
            if std::fs::rename(&plugin_dir, &moved_plugin_dir).is_ok() {
                create_directory_link(&outside_dir, &plugin_dir);
            }
        },
    );

    assert_eq!(
        result.unwrap(),
        b"trusted",
        "bounded read followed a swapped parent link",
    );
}

#[test]
fn bounded_read_does_not_follow_a_final_component_swapped_after_validation() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    let plugin_dir = plugins_dir.join("race-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest_path = plugin_dir.join("race-plugin.toml");
    let outside_manifest = temp_dir.path().join("outside.toml");
    std::fs::write(&manifest_path, b"trusted").unwrap();
    std::fs::write(&outside_manifest, b"outside").unwrap();
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let result = loader.read_plugin_file_bounded_with_hook(
        &manifest_path,
        MAX_PLUGIN_MANIFEST_BYTES,
        "plugin manifest",
        || {
            std::fs::remove_file(&manifest_path).unwrap();
            if !create_file_link(&outside_manifest, &manifest_path) {
                std::fs::create_dir(&manifest_path).unwrap();
            }
        },
    );

    assert!(
        result.is_err(),
        "bounded read followed a swapped final component"
    );
}

#[test]
fn selected_package_read_rejects_a_final_component_replaced_before_open() {
    let temp_dir = TempDir::new().unwrap();
    let plugins_dir = temp_dir.path().join("plugins");
    std::fs::create_dir(&plugins_dir).unwrap();
    let selected = temp_dir.path().join("selected.wirt");
    let outside = temp_dir.path().join("outside.wirt");
    std::fs::write(&selected, b"selected").unwrap();
    std::fs::write(&outside, b"outside").unwrap();
    let loader = PluginLoader::new(plugins_dir).unwrap();

    let result = loader.read_package_file_with_hook(&selected, || {
        std::fs::remove_file(&selected).unwrap();
        if !create_file_link(&outside, &selected) {
            std::fs::create_dir(&selected).unwrap();
        }
    });

    assert!(result.is_err(), "selected package replacement was followed");
}
