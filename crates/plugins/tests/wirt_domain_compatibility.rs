use std::collections::HashMap;

use arclain_plugins::PluginManager;
use tempfile::TempDir;

// The manifest is pinned beside the component rather than read from
// `plugins/dlsite-metadata/`. Both halves describe one frozen artifact,
// and a package only loads when they agree on its version -- so pointing
// at the live manifest made this legacy check fail the moment the plugin
// was version-bumped, for a reason that had nothing to do with the host
// contract it exists to verify.
const DLSITE_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wirt/dlsite-metadata.plugin.toml"
));
const DLSITE_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wirt/dlsite-metadata.wasm"
));

#[test]
fn legacy_dlsite_guest_retains_its_arclain_domain_contract() {
    let plugins = TempDir::new().unwrap();
    let package = wirt::package_bytes(DLSITE_MANIFEST, DLSITE_COMPONENT).unwrap();
    std::fs::write(plugins.path().join("dlsite-metadata.wirt"), package).unwrap();

    let mut manager = PluginManager::new(plugins.path().to_path_buf(), HashMap::new()).unwrap();
    manager.init().unwrap();

    let metadata = manager
        .execute_plugin("dlsite-metadata", wirt::ExecutorRequest::Metadata)
        .unwrap()
        .into_metadata()
        .unwrap();
    assert_eq!(metadata.id, "dlsite-metadata");

    let rules = manager
        .execute_plugin("dlsite-metadata", wirt::ExecutorRequest::DefaultRules)
        .unwrap()
        .into_rules()
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].trigger.metadata_source.as_deref(), Some("dlsite"));
}

#[test]
fn legacy_dlsite_package_rejects_a_mismatched_component_abi_before_init() {
    let mut mismatched = DLSITE_COMPONENT.to_vec();
    let mut replacements = 0;
    let mut offset = 0;
    while let Some(found) = mismatched[offset..]
        .windows(b"@0.3.0".len())
        .position(|window| window == b"@0.3.0")
    {
        let start = offset + found;
        mismatched[start..start + b"@0.3.0".len()].copy_from_slice(b"@0.2.0");
        replacements += 1;
        offset = start + b"@0.3.0".len();
    }
    assert!(
        replacements > 0,
        "fixture did not expose its versioned imports"
    );

    let error = wirt::package_bytes(DLSITE_MANIFEST, &mismatched)
        .expect_err("component preflight must reject a mismatched ABI")
        .to_string();
    assert!(error.contains("component-preflight"), "{error}");
    assert!(error.contains("0.2.0"), "{error}");
    assert!(error.contains(wirt::WIRT_ABI_VERSION), "{error}");
}
