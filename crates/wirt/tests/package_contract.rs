mod support;

use std::collections::BTreeSet;
use std::io::Cursor;
use support::{manifest_toml, UI_DEMO_COMPONENT};
use wirt::{
    inspect_component_contract, package_bytes, read_package_bytes, PackageFingerprint,
    WIRT_ABI_VERSION,
};
use zip::{CompressionMethod, ZipArchive};

#[test]
fn package_bytes_are_deterministic_and_round_trip_exact_inputs() {
    let first = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let second = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    assert_eq!(first, second);

    let package = read_package_bytes(&first).unwrap();
    assert_eq!(package.manifest.wirt.abi, WIRT_ABI_VERSION);
    assert_eq!(package.manifest_bytes, manifest_toml().as_bytes());
    assert_eq!(package.component, UI_DEMO_COMPONENT);
    assert_eq!(package.fingerprint, PackageFingerprint::sha256(&first));
}

#[test]
fn package_bytes_have_the_canonical_two_entry_zip_layout() {
    let bytes = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();

    assert_eq!(archive.len(), 2);
    assert!(archive.comment().is_empty());
    for (index, expected_name) in ["plugin.toml", "plugin.wasm"].iter().enumerate() {
        let entry = archive.by_index(index).unwrap();
        assert_eq!(entry.name_raw(), expected_name.as_bytes());
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        assert_eq!(entry.unix_mode(), Some(0o100644));
        assert!(entry.comment().is_empty());
        assert!(entry.extra_data().unwrap_or_default().is_empty());
        let modified = entry.last_modified().unwrap();
        assert_eq!(modified.year(), 1980);
        assert_eq!(modified.month(), 1);
        assert_eq!(modified.day(), 1);
        assert_eq!(modified.hour(), 0);
        assert_eq!(modified.minute(), 0);
        assert_eq!(modified.second(), 0);
    }
}

#[test]
fn canonical_component_has_only_the_fixed_wirt_and_wasi_contract() {
    let contract = inspect_component_contract(UI_DEMO_COMPONENT).unwrap();
    assert_eq!(contract.abi, WIRT_ABI_VERSION);
    assert_eq!(
        contract.exports,
        [
            "get-default-rules",
            "get-metadata",
            "get-top-tabs",
            "get-ui-layout",
            "init",
            "on-ui-event",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        contract.imports,
        [
            "wasi:cli/environment@0.2.9",
            "wasi:cli/exit@0.2.9",
            "wasi:cli/stderr@0.2.9",
            "wasi:cli/stdin@0.2.9",
            "wasi:cli/stdout@0.2.9",
            "wasi:cli/terminal-input@0.2.9",
            "wasi:cli/terminal-output@0.2.9",
            "wasi:cli/terminal-stderr@0.2.9",
            "wasi:cli/terminal-stdin@0.2.9",
            "wasi:cli/terminal-stdout@0.2.9",
            "wasi:clocks/monotonic-clock@0.2.9",
            "wasi:io/error@0.2.9",
            "wasi:io/poll@0.2.9",
            "wasi:io/streams@0.2.9",
            "wirt:plugin/host@0.1.0",
            "wirt:plugin/meta@0.1.0",
            "wirt:plugin/rules@0.1.0",
            "wirt:plugin/ui@0.1.0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    );
}

#[test]
fn payload_strings_cannot_spoof_component_imports() {
    let payload = b"wirt:plugin/host@0.1.0 wasi:io/poll@0.2.9";
    let mut bytes = b"\0asm\x0d\0\x01\0".to_vec();
    bytes.push(0);
    bytes.push((payload.len() + 1) as u8);
    bytes.push(0);
    bytes.extend_from_slice(payload);

    let error = inspect_component_contract(&bytes).unwrap_err();
    assert!(error.to_string().contains("required Wirt import"));
}
