mod support;

use std::collections::BTreeSet;
use std::io::Cursor;
use support::{manifest_toml, UI_DEMO_COMPONENT};
use wirt::{
    inspect_component_contract, package_bytes, read_package_bytes, PackageFingerprint,
    WIRT_ABI_VERSION,
};
use zip::{CompressionMethod, ZipArchive};

const FACADE_TEST_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/facade-test-fixture/facade-test-fixture.wasm"
));
const FACADE_TEST_MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/facade-test-fixture/plugin.toml"
));
const FACADE_TEST_PACKAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/bundled/facade-test-fixture.wirt"
));

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
    swap: Option<(&'static str, &'static str)>,
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
            let name = match self.swap {
                Some((first, second)) if original == first => second,
                Some((first, second)) if original == second => first,
                _ => name,
            };
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

struct InstanceMutation {
    marker: &'static str,
    fresh_resource_export: Option<&'static str>,
    extra_export_name: Option<String>,
}

impl wasm_encoder::reencode::Reencode for InstanceMutation {
    type Error = std::convert::Infallible;
}

impl wasm_encoder::reencode::ReencodeComponent for InstanceMutation {
    fn component_instance_type(
        &mut self,
        declarations: Box<[wasmparser::InstanceTypeDeclaration<'_>]>,
    ) -> Result<wasm_encoder::InstanceType, wasm_encoder::reencode::Error<Self::Error>> {
        use wasm_encoder::{ComponentTypeRef, InstanceType, PrimitiveValType, TypeBounds};
        use wasmparser::InstanceTypeDeclaration;

        let selected = declarations.iter().any(|declaration| {
            matches!(
                declaration,
                InstanceTypeDeclaration::Export { name, .. } if name.name == self.marker
            )
        });
        let mut instance = InstanceType::new();
        for declaration in declarations {
            if selected {
                if let (Some(target), InstanceTypeDeclaration::Export { name, .. }) =
                    (self.fresh_resource_export, &declaration)
                {
                    if name.name == target {
                        instance.export(name.name, ComponentTypeRef::Type(TypeBounds::SubResource));
                        continue;
                    }
                }
            }
            self.parse_component_instance_type_declaration(&mut instance, declaration)?;
        }
        if selected && self.fresh_resource_export.is_none() {
            let index = instance.type_count();
            instance
                .ty()
                .defined_type()
                .primitive(PrimitiveValType::U32);
            instance.export(
                self.extra_export_name
                    .as_deref()
                    .unwrap_or("review-extra-type"),
                ComponentTypeRef::Type(TypeBounds::Eq(index)),
            );
        }
        Ok(instance)
    }
}

fn mutate_instance_type_in(component_bytes: &[u8], mutation: InstanceMutation) -> Vec<u8> {
    use wasm_encoder::reencode::ReencodeComponent;

    let mut mutation = mutation;
    let mut component = wasm_encoder::Component::new();
    mutation
        .parse_component(&mut component, wasmparser::Parser::new(0), component_bytes)
        .unwrap();
    component.finish()
}

fn mutate_instance_type(mutation: InstanceMutation) -> Vec<u8> {
    mutate_instance_type_in(UI_DEMO_COMPONENT, mutation)
}

struct PublicTypeNameSwap {
    first: &'static str,
    second: &'static str,
}

impl wasm_encoder::reencode::Reencode for PublicTypeNameSwap {
    type Error = std::convert::Infallible;
}

impl wasm_encoder::reencode::ReencodeComponent for PublicTypeNameSwap {
    fn parse_component_import_section(
        &mut self,
        imports: &mut wasm_encoder::ComponentImportSection,
        section: wasmparser::ComponentImportSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
        for import in section {
            let import = import?;
            let name = match import.name.name {
                name if name == self.first => self.second,
                name if name == self.second => self.first,
                name => name,
            };
            imports.import(name, self.component_type_ref(import.ty)?);
        }
        Ok(())
    }
}

fn swap_public_type_names(first: &'static str, second: &'static str) -> Vec<u8> {
    use wasm_encoder::reencode::ReencodeComponent;

    let mut mutation = PublicTypeNameSwap { first, second };
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

#[derive(Default)]
struct DuplicateMetadataRecord {
    depth: usize,
    inserted: bool,
    duplicate_index: u32,
}

impl DuplicateMetadataRecord {
    fn mapped_top_type(&self, index: u32) -> u32 {
        if self.inserted && index >= self.duplicate_index {
            index + 1
        } else {
            index
        }
    }
}

impl wasm_encoder::reencode::Reencode for DuplicateMetadataRecord {
    type Error = std::convert::Infallible;
}

impl wasm_encoder::reencode::ReencodeComponent for DuplicateMetadataRecord {
    fn push_depth(&mut self) {
        self.depth += 1;
    }

    fn pop_depth(&mut self) {
        self.depth -= 1;
    }

    fn component_type_index(&mut self, index: u32) -> u32 {
        if self.depth == 0 {
            self.mapped_top_type(index)
        } else {
            index
        }
    }

    fn outer_component_type_index(&mut self, count: u32, index: u32) -> u32 {
        if count as usize == self.depth {
            self.mapped_top_type(index)
        } else {
            index
        }
    }

    fn parse_component_type_section(
        &mut self,
        types: &mut wasm_encoder::ComponentTypeSection,
        section: wasmparser::ComponentTypeSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
        wasm_encoder::reencode::component_utils::parse_component_type_section(
            self, types, section,
        )?;
        if self.depth == 0 && !self.inserted {
            self.duplicate_index = types.len();
            types.defined_type().record([
                ("id", wasm_encoder::PrimitiveValType::String),
                ("name", wasm_encoder::PrimitiveValType::String),
                ("version", wasm_encoder::PrimitiveValType::String),
                ("author", wasm_encoder::PrimitiveValType::String),
                ("description", wasm_encoder::PrimitiveValType::String),
            ]);
            self.inserted = true;
        }
        Ok(())
    }

    fn parse_component_import_section(
        &mut self,
        imports: &mut wasm_encoder::ComponentImportSection,
        section: wasmparser::ComponentImportSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error<Self::Error>> {
        for import in section {
            let import = import?;
            let ty = if self.depth == 0 && import.name.name == "plugin-metadata" {
                wasm_encoder::ComponentTypeRef::Type(wasm_encoder::TypeBounds::Eq(
                    self.duplicate_index,
                ))
            } else {
                self.component_type_ref(import.ty)?
            };
            imports.import(import.name, ty);
        }
        Ok(())
    }
}

fn component_with_duplicated_metadata_record() -> Vec<u8> {
    use wasm_encoder::reencode::ReencodeComponent;

    let mut mutation = DuplicateMetadataRecord::default();
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

fn assert_contract_type_rejection(label: &str, component: &[u8]) {
    wasmparser::Validator::new()
        .validate_all(component)
        .unwrap_or_else(|error| panic!("{label} fixture was not validator-valid: {error}"));
    let error = inspect_component_contract(component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: contract-type-mismatch"),
        "unexpected classification for {label}: {error}"
    );
}

fn deeply_nested_contract(depth: u32) -> Vec<u8> {
    use wasm_encoder::{
        Component, ComponentExportKind, ComponentExportSection, ComponentImportSection,
        ComponentTypeRef, ComponentTypeSection, ComponentValType, InstanceType, PrimitiveValType,
        TypeBounds,
    };

    const INTERFACES: [&str; 18] = [
        "wirt:plugin/host@0.2.0",
        "wirt:plugin/meta@0.2.0",
        "wirt:plugin/rules@0.2.0",
        "wirt:plugin/ui@0.2.0",
        "wasi:io/poll@0.2.9",
        "wasi:clocks/monotonic-clock@0.2.9",
        "wasi:io/error@0.2.9",
        "wasi:io/streams@0.2.9",
        "wasi:cli/stdout@0.2.9",
        "wasi:cli/stderr@0.2.9",
        "wasi:cli/stdin@0.2.9",
        "wasi:cli/environment@0.2.9",
        "wasi:cli/exit@0.2.9",
        "wasi:cli/terminal-input@0.2.9",
        "wasi:cli/terminal-output@0.2.9",
        "wasi:cli/terminal-stdin@0.2.9",
        "wasi:cli/terminal-stdout@0.2.9",
        "wasi:cli/terminal-stderr@0.2.9",
    ];
    const PUBLIC_TYPES: [&str; 6] = [
        "plugin-action",
        "plugin-layout",
        "plugin-metadata",
        "plugin-rule-definition",
        "top-tab-config",
        "ui-element",
    ];
    const EXPORTS: [&str; 6] = [
        "init",
        "get-default-rules",
        "get-ui-layout",
        "on-ui-event",
        "get-top-tabs",
        "get-metadata",
    ];

    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    types.defined_type().primitive(PrimitiveValType::U32);
    let mut nested_index = 1;
    for _ in 0..depth {
        types
            .defined_type()
            .option(ComponentValType::Type(nested_index));
        nested_index += 1;
    }
    component.section(&types);

    let mut imports = ComponentImportSection::new();
    for name in INTERFACES {
        imports.import(name, ComponentTypeRef::Instance(0));
    }
    let first_imported_type = nested_index + 1;
    for name in PUBLIC_TYPES {
        imports.import(name, ComponentTypeRef::Type(TypeBounds::Eq(nested_index)));
    }
    component.section(&imports);

    let mut exports = ComponentExportSection::new();
    for (offset, name) in EXPORTS.into_iter().enumerate() {
        exports.export(
            name,
            ComponentExportKind::Type,
            first_imported_type + offset as u32,
            None,
        );
    }
    component.section(&exports);
    component.finish()
}

fn wide_contract(field_count: usize) -> Vec<u8> {
    use wasm_encoder::{
        Component, ComponentExportKind, ComponentExportSection, ComponentImportSection,
        ComponentTypeRef, ComponentTypeSection, ComponentValType, InstanceType, PrimitiveValType,
        TypeBounds,
    };

    const INTERFACES: [&str; 18] = [
        "wirt:plugin/host@0.2.0",
        "wirt:plugin/meta@0.2.0",
        "wirt:plugin/rules@0.2.0",
        "wirt:plugin/ui@0.2.0",
        "wasi:io/poll@0.2.9",
        "wasi:clocks/monotonic-clock@0.2.9",
        "wasi:io/error@0.2.9",
        "wasi:io/streams@0.2.9",
        "wasi:cli/stdout@0.2.9",
        "wasi:cli/stderr@0.2.9",
        "wasi:cli/stdin@0.2.9",
        "wasi:cli/environment@0.2.9",
        "wasi:cli/exit@0.2.9",
        "wasi:cli/terminal-input@0.2.9",
        "wasi:cli/terminal-output@0.2.9",
        "wasi:cli/terminal-stdin@0.2.9",
        "wasi:cli/terminal-stdout@0.2.9",
        "wasi:cli/terminal-stderr@0.2.9",
    ];
    const PUBLIC_TYPES: [&str; 6] = [
        "plugin-action",
        "plugin-layout",
        "plugin-metadata",
        "plugin-rule-definition",
        "top-tab-config",
        "ui-element",
    ];
    const EXPORTS: [&str; 6] = [
        "init",
        "get-default-rules",
        "get-ui-layout",
        "on-ui-event",
        "get-top-tabs",
        "get-metadata",
    ];

    assert_eq!(field_count % PUBLIC_TYPES.len(), 0);
    let tuple_width = field_count / PUBLIC_TYPES.len();
    assert!(tuple_width <= 10_000);

    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    types.defined_type().primitive(PrimitiveValType::U32);
    for _ in PUBLIC_TYPES {
        types
            .defined_type()
            .tuple(std::iter::repeat_n(ComponentValType::Type(1), tuple_width));
    }
    component.section(&types);

    let mut imports = ComponentImportSection::new();
    for name in INTERFACES {
        imports.import(name, ComponentTypeRef::Instance(0));
    }
    let first_imported_type = PUBLIC_TYPES.len() as u32 + 2;
    for (index, name) in PUBLIC_TYPES.into_iter().enumerate() {
        imports.import(
            name,
            ComponentTypeRef::Type(TypeBounds::Eq(index as u32 + 2)),
        );
    }
    component.section(&imports);

    let mut exports = ComponentExportSection::new();
    for (offset, name) in EXPORTS.into_iter().enumerate() {
        exports.export(
            name,
            ComponentExportKind::Type,
            first_imported_type + offset as u32,
            None,
        );
    }
    component.section(&exports);
    component.finish()
}

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
fn maintained_facade_package_matches_its_manifest_and_component() {
    let rebuilt = package_bytes(FACADE_TEST_MANIFEST, FACADE_TEST_COMPONENT).unwrap();
    assert_eq!(rebuilt, FACADE_TEST_PACKAGE);

    let package = read_package_bytes(FACADE_TEST_PACKAGE).unwrap();
    assert_eq!(package.manifest_bytes, FACADE_TEST_MANIFEST);
    assert_eq!(package.component, FACADE_TEST_COMPONENT);
    assert_eq!(
        package.fingerprint,
        PackageFingerprint::sha256(FACADE_TEST_PACKAGE)
    );
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
            "wirt:plugin/host@0.2.0",
            "wirt:plugin/meta@0.2.0",
            "wirt:plugin/rules@0.2.0",
            "wirt:plugin/ui@0.2.0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    );
}

#[test]
fn structural_preflight_accepts_duplicated_equivalent_nonresource_types() {
    let component = component_with_duplicated_metadata_record();
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let contract = inspect_component_contract(&component).unwrap();
    assert_eq!(contract.abi, WIRT_ABI_VERSION);
}

#[test]
fn payload_strings_cannot_spoof_component_imports() {
    let payload = b"wirt:plugin/host@0.2.0 wasi:io/poll@0.2.9";
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
        "wirt:plugin/host@0.1.0",
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
        error.contains("component-preflight: contract-type-mismatch"),
        "unexpected classification: {error}"
    );
}

#[test]
fn structural_preflight_rejects_wrong_wirt_interface_type() {
    let component = mutate_instance_type(InstanceMutation {
        marker: "log",
        fresh_resource_export: None,
        extra_export_name: None,
    });
    assert_contract_type_rejection("wirt host", &component);
}

#[test]
fn structural_preflight_accepts_canonical_sdk_interface_member_subsets() {
    inspect_component_contract(UI_DEMO_COMPONENT)
        .expect("the SDK guest that imports only host.log must be accepted");
    inspect_component_contract(FACADE_TEST_COMPONENT)
        .expect("the SDK guest that imports a larger canonical host subset must be accepted");
}

#[test]
fn structural_preflight_accepts_canonical_member_unused_by_bundled_guests() {
    let types = wasmparser::Validator::new()
        .validate_all(FACADE_TEST_COMPONENT)
        .expect("the facade fixture is a valid component");
    let host = types
        .component_item_for_import("wirt:plugin/host@0.2.0")
        .expect("the facade fixture imports the Wirt host interface");
    let wasmparser::component_types::ComponentEntityType::Instance(host) = host.ty else {
        panic!("the Wirt host import is an instance");
    };
    assert!(
        types[host].exports.contains_key("create-file"),
        "the fixture must exercise a canonical host member omitted by bundled guests"
    );

    inspect_component_contract(FACADE_TEST_COMPONENT).expect(
        "the external SDK fixture's create-file import is canonical even though bundled guests omit it",
    );
}

#[test]
fn structural_preflight_keeps_member_and_resource_checks_for_subsets() {
    let cases = [
        (
            "unknown host member",
            mutate_instance_type(InstanceMutation {
                marker: "log",
                fresh_resource_export: None,
                extra_export_name: Some("mine-bitcoin".to_string()),
            }),
        ),
        (
            "wrong canonical host member type",
            mutate_instance_type(InstanceMutation {
                marker: "log",
                fresh_resource_export: None,
                extra_export_name: Some("get-setting".to_string()),
            }),
        ),
        (
            "fresh cross-interface resource",
            mutate_instance_type(InstanceMutation {
                marker: "get-stdout",
                fresh_resource_export: Some("output-stream"),
                extra_export_name: None,
            }),
        ),
    ];

    for (label, component) in cases {
        assert_contract_type_rejection(label, &component);
    }
}

#[test]
fn structural_preflight_rejects_wrong_type_for_every_allowed_interface() {
    for (name, marker) in [
        ("wirt:plugin/rules@0.2.0", "plugin-rule-definition"),
        ("wirt:plugin/ui@0.2.0", "ui-element"),
        ("wirt:plugin/meta@0.2.0", "plugin-metadata"),
        ("wirt:plugin/host@0.2.0", "log"),
        ("wasi:io/poll@0.2.9", "poll"),
        ("wasi:clocks/monotonic-clock@0.2.9", "subscribe-duration"),
        ("wasi:io/error@0.2.9", "error"),
        ("wasi:io/streams@0.2.9", "stream-error"),
        ("wasi:cli/stdout@0.2.9", "get-stdout"),
        ("wasi:cli/stderr@0.2.9", "get-stderr"),
        ("wasi:cli/stdin@0.2.9", "get-stdin"),
        ("wasi:cli/environment@0.2.9", "get-environment"),
        ("wasi:cli/exit@0.2.9", "exit"),
        ("wasi:cli/terminal-input@0.2.9", "terminal-input"),
        ("wasi:cli/terminal-output@0.2.9", "terminal-output"),
        ("wasi:cli/terminal-stdin@0.2.9", "get-terminal-stdin"),
        ("wasi:cli/terminal-stdout@0.2.9", "get-terminal-stdout"),
        ("wasi:cli/terminal-stderr@0.2.9", "get-terminal-stderr"),
    ] {
        let component = mutate_instance_type(InstanceMutation {
            marker,
            fresh_resource_export: None,
            extra_export_name: None,
        });
        assert_contract_type_rejection(name, &component);
    }
}

#[test]
fn structural_preflight_rejects_every_public_type_and_export_mutation() {
    let public_types = [
        "plugin-action",
        "plugin-layout",
        "plugin-metadata",
        "plugin-rule-definition",
        "top-tab-config",
        "ui-element",
    ];
    for (index, name) in public_types.iter().enumerate() {
        let replacement = public_types[(index + 1) % public_types.len()];
        let component = swap_public_type_names(name, replacement);
        assert_contract_type_rejection(name, &component);
    }

    let exports = [
        "init",
        "get-default-rules",
        "get-ui-layout",
        "on-ui-event",
        "get-top-tabs",
        "get-metadata",
    ];
    for (index, name) in exports.iter().enumerate() {
        let replacement = exports[(index + 1) % exports.len()];
        let component = mutate_exports(ExportMutation {
            swap: Some((name, replacement)),
            ..ExportMutation::default()
        });
        assert_contract_type_rejection(name, &component);
    }
}

#[test]
fn structural_preflight_rejects_shape_identical_fresh_cross_interface_resources() {
    for (label, marker, resource) in [
        ("streams-to-stdout", "get-stdout", "output-stream"),
        ("streams-to-stderr", "get-stderr", "output-stream"),
        ("poll-to-streams", "stream-error", "pollable"),
        ("error-to-streams", "stream-error", "error"),
    ] {
        let component = mutate_instance_type(InstanceMutation {
            marker,
            fresh_resource_export: Some(resource),
            extra_export_name: None,
        });
        assert_contract_type_rejection(label, &component);
    }
}

#[test]
fn structural_preflight_bounds_total_hashed_identifier_bytes() {
    let component = mutate_instance_type(InstanceMutation {
        marker: "log",
        fresh_resource_export: None,
        extra_export_name: Some("hostile".repeat(13_000)),
    });
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: type-complexity"),
        "unexpected classification: {error}"
    );
    assert!(error.len() <= 240, "unbounded diagnostic: {error}");
}

#[test]
fn structural_preflight_bounds_owned_top_level_name_bytes() {
    let name = "hostile".repeat(13_000);
    let component = component_with_empty_interface_import(&name);
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: type-complexity"),
        "unexpected classification: {error}"
    );
    assert!(error.len() <= 240, "unbounded diagnostic: {error}");
}

#[test]
fn structural_preflight_bounds_deeply_nested_valid_type_graphs_iteratively() {
    let component = deeply_nested_contract(80);
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: type-complexity"),
        "unexpected classification: {error}"
    );
}

#[test]
fn structural_preflight_bounds_wide_valid_type_graphs_before_queue_growth() {
    let component = wide_contract(60_000);
    wasmparser::Validator::new()
        .validate_all(&component)
        .unwrap();
    let error = inspect_component_contract(&component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("component-preflight: type-complexity"),
        "unexpected classification: {error}"
    );
}
