mod support;

use std::collections::BTreeSet;
use std::io::Cursor;
use support::{manifest_toml, UI_DEMO_COMPONENT};
use wirt::{
    inspect_component_contract, package_bytes, read_package_bytes, PackageFingerprint,
    WIRT_ABI_VERSION,
};
use zip::{CompressionMethod, ZipArchive};

fn component_with_empty_interface_import(name: &str) -> Vec<u8> {
    use wasm_encoder::{
        Component, ComponentImportSection, ComponentTypeRef, ComponentTypeSection, InstanceType,
    };

    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    component.section(&types);
    let mut imports = ComponentImportSection::new();
    imports.import(name, ComponentTypeRef::Instance(0));
    component.section(&imports);
    component.finish()
}

#[derive(Default)]
struct ExportMutation {
    rename: Option<(&'static str, &'static str)>,
    duplicate: Option<(&'static str, &'static str)>,
    replace_index: Option<(&'static str, u32)>,
}

impl wasm_encoder::reencode::Reencode for ExportMutation {
    type Error = std::convert::Infallible;
}

impl wasm_encoder::reencode::ReencodeComponent for ExportMutation {
    fn parse_component_export_section(
        &mut self,
        exports: &mut wasm_encoder::ComponentExportSection,
        section: wasmparser::ComponentExportSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
        for export in section {
            let export = export?;
            let original = export.name.name;
            let name = self
                .rename
                .filter(|(from, _)| *from == original)
                .map_or(original, |(_, to)| to);
            let index = self
                .replace_index
                .filter(|(target, _)| *target == original)
                .map_or(export.index, |(_, replacement)| replacement);
            exports.export(
                name,
                export.kind.into(),
                self.component_external_index(export.kind, index),
                export
                    .ty
                    .map(|ty| self.component_type_ref(ty))
                    .transpose()?,
            );
            if let Some((target, extra)) = self.duplicate.filter(|(target, _)| *target == original)
            {
                let _ = target;
                exports.export(
                    extra,
                    export.kind.into(),
                    self.component_external_index(export.kind, export.index),
                    export
                        .ty
                        .map(|ty| self.component_type_ref(ty))
                        .transpose()?,
                );
            }
        }
        Ok(())
    }
}

fn mutate_exports(mutation: ExportMutation) -> Vec<u8> {
    use wasm_encoder::reencode::ReencodeComponent;

    let mut mutation = mutation;
    let mut component = wasm_encoder::Component::new();
    mutation
        .parse_component(
            &mut component,
            wasmparser::Parser::new(0),
            UI_DEMO_COMPONENT,
        )
        .unwrap();
    component.finish()
}

#[test]
fn package_bytes_are_deterministic_and_round_trip_exact_inputs() {
    let first = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let second = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        PackageFingerprint::sha256(&first).as_str(),
        "d06666940093ae934b664d1447ae7f1d03f2a2a21533857348a031d5a9b17dc7"
    );

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

#[test]
fn structural_preflight_rejects_nonallowlisted_interface_names() {
    for name in [
        "attacker:coin/miner@1.0.0",
        "wasi:sockets/tcp@0.2.9",
        "wasi:filesystem/types@0.2.9",
        "wasi:random/random@0.2.9",
    ] {
        let error = inspect_component_contract(&component_with_empty_interface_import(name))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("component-preflight: unsupported import"),
            "unexpected classification for {name:?}: {error}"
        );
    }
}

#[test]
fn structural_preflight_rejects_wrong_wirt_interface_version() {
    let error = inspect_component_contract(&component_with_empty_interface_import(
        "wirt:plugin/host@0.2.0",
    ))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("component-preflight: unsupported Wirt interface version"),
        "unexpected classification: {error}"
    );
}

#[test]
fn structural_preflight_bounds_hostile_component_names() {
    let name = format!("attacker:coin/{}@1.0.0", "miner".repeat(1_000));
    let error = inspect_component_contract(&component_with_empty_interface_import(&name))
        .unwrap_err()
        .to_string();
    assert!(
        error.len() <= 240,
        "component error was unbounded: {} bytes",
        error.len()
    );
    assert!(
        !error.contains(&name),
        "component error reflected the full hostile name"
    );
}

#[test]
fn structural_preflight_bounds_direct_component_inspection() {
    let component = vec![0_u8; wirt::MAX_PLUGIN_WASM_BYTES + 1];
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(error.contains("component-preflight: component exceeds"));
}

#[test]
fn structural_preflight_rejects_missing_and_additional_exports() {
    for component in [
        mutate_exports(ExportMutation {
            rename: Some(("init", "evil")),
            ..ExportMutation::default()
        }),
        mutate_exports(ExportMutation {
            duplicate: Some(("init", "evil")),
            ..ExportMutation::default()
        }),
    ] {
        wasmparser::Validator::new()
            .validate_all(&component)
            .unwrap();
        let error = inspect_component_contract(&component)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("component-preflight: exports do not match"),
            "unexpected classification: {error}"
        );
    }
}

#[test]
fn structural_preflight_rejects_wrong_export_parameter_and_result_types() {
    let component = mutate_exports(ExportMutation {
        // Re-export the no-parameter list-returning function under the
        // string-parameter/layout-result name. The component remains valid,
        // but both the canonical parameters and result are wrong.
        replace_index: Some(("get-ui-layout", 19)),
        ..ExportMutation::default()
    });
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: export type mismatch for \"get-ui-layout\""),
        "unexpected classification: {error}"
    );
}

#[test]
fn structural_preflight_rejects_wrong_wirt_interface_type() {
    use wasm_encoder::{
        Component, ComponentImportSection, ComponentTypeRef, ComponentTypeSection, InstanceType,
    };

    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    component.section(&types);
    let mut imports = ComponentImportSection::new();
    for name in [
        "wirt:plugin/host@0.1.0",
        "wirt:plugin/meta@0.1.0",
        "wirt:plugin/rules@0.1.0",
        "wirt:plugin/ui@0.1.0",
    ] {
        imports.import(name, ComponentTypeRef::Instance(0));
    }
    component.section(&imports);
    let component = component.finish();
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: import type mismatch"),
        "unexpected classification: {error}"
    );
}
