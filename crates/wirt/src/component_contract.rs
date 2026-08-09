use crate::{PluginError, Result, MAX_PLUGIN_WASM_BYTES, WIRT_ABI_VERSION};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use wasmparser::component_types::{
    ComponentAnyTypeId, ComponentDefinedType, ComponentEntityType, ComponentValType, ResourceId,
};
use wasmparser::types::Types;
use wasmparser::{Encoding, Parser, Payload, Validator};

const WIRT_IMPORTS: [&str; 4] = [
    "wirt:plugin/host@0.1.0",
    "wirt:plugin/meta@0.1.0",
    "wirt:plugin/rules@0.1.0",
    "wirt:plugin/ui@0.1.0",
];

const WASI_IMPORTS: [&str; 14] = [
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

const EXPORTS: [&str; 6] = [
    "init",
    "get-default-rules",
    "get-ui-layout",
    "on-ui-event",
    "get-top-tabs",
    "get-metadata",
];

// Filled from the checked-in canonical WIT-generated fixture. These hashes are
// over wasmparser's validated structural type graph, not component bytes.
const IMPORT_TYPE_HASHES: [(&str, &str); 18] = [
    (
        "wirt:plugin/host@0.1.0",
        "54e07862b243e8ce1e189e6a7ad6216a9d1f4cd5e122ab43980e19e3098e8bb6",
    ),
    (
        "wirt:plugin/meta@0.1.0",
        "93f9d4a9954319181494d40caf46e6cb1484e1ed7a4b4988592de23f4e518cbd",
    ),
    (
        "wirt:plugin/rules@0.1.0",
        "631cb759865da17c06c15a96eab9a3f2b9b6efecd5733a2265f56288332d6a34",
    ),
    (
        "wirt:plugin/ui@0.1.0",
        "dada9eec1053c0fe5a43c249d6b551bf8049950a88d102736f80b298767b3651",
    ),
    (
        "wasi:io/poll@0.2.9",
        "70352c2e839da4051dd94af143f43a645767d55b29a261cd7a5a386d3495368b",
    ),
    (
        "wasi:clocks/monotonic-clock@0.2.9",
        "d66b200924daac76e726c86576d3fb9538028d32eb9771e8e4a9a07d138bb524",
    ),
    (
        "wasi:io/error@0.2.9",
        "255ce99c75ac5da82735823124103ac563c5e2cd5f41672e38e514bfe323c05c",
    ),
    (
        "wasi:io/streams@0.2.9",
        "758ae848426220768c35acec578e743a660eafc680006194e817d5427971b6cc",
    ),
    (
        "wasi:cli/stdout@0.2.9",
        "164690e29127d1dc4101693c91bdebfa73ee61aa5c0ed22af71a7023482346dc",
    ),
    (
        "wasi:cli/stderr@0.2.9",
        "3bc0220828a2e9aff5e69ecb04e0478dfb4f30e96060aa918e064ee56e65ac47",
    ),
    (
        "wasi:cli/stdin@0.2.9",
        "21ca147eafcb246ab40836fa27d25c71d8822b400fcaa154c08862365870c694",
    ),
    (
        "wasi:cli/environment@0.2.9",
        "c493396da31ff300615dbe593692c28c7108eb722289b18733c5508bb689a8f9",
    ),
    (
        "wasi:cli/exit@0.2.9",
        "bae03c797a04e1430d000f14af0ca2f96fd22c6f561a2ce4951d53a52492e863",
    ),
    (
        "wasi:cli/terminal-input@0.2.9",
        "0badd57a959756b2fe63927cf61cf8cb4e266ddd90c0532a9001122eac32816f",
    ),
    (
        "wasi:cli/terminal-output@0.2.9",
        "9a9edc9779e874e4ba90601d7cb28415548c177e3c7e5d2aff9b14c9397c4b12",
    ),
    (
        "wasi:cli/terminal-stdin@0.2.9",
        "5ac5d95b81f1ab0a2543c7eb1bfff1ae497d75de1a67217fba7c416602470597",
    ),
    (
        "wasi:cli/terminal-stdout@0.2.9",
        "2ab587ad2c11f68ac13e5b754a680ea767c74b9908b2a2be2f7ee03fbf9592b2",
    ),
    (
        "wasi:cli/terminal-stderr@0.2.9",
        "3023a5730718dbca064e5a1829a02df45fdaad3535fe064644089db05b8c967c",
    ),
];

const TYPE_IMPORT_HASHES: [(&str, &str); 6] = [
    (
        "plugin-action",
        "d0c1f99ca61e9dd9d3cd62d0dd547cd752d8f78b7a0a8cced4ca38f36b1ecb75",
    ),
    (
        "plugin-layout",
        "6f0d03bb9af25f6141c42dfdf9a3843d72a58cd21ccaafd8e49f6f409af33086",
    ),
    (
        "plugin-metadata",
        "691129c5a056597d5858478fbe27c79626d6aa2d16b24f398c3b8a9b6a7b8b0f",
    ),
    (
        "plugin-rule-definition",
        "e7c9361962ea20f0a8c3f437b05d6da4e17f8dd9c4bcd77dd448dccb58652ee9",
    ),
    (
        "top-tab-config",
        "3b370f9acdbb966002ba4cde8029de105122614e699b8c079940dcc47a0b1cc3",
    ),
    (
        "ui-element",
        "2f2656576cabffcbcda38b45900aa76d4ae835d1a93e71da7911ade6551bdd53",
    ),
];

const EXPORT_TYPE_HASHES: [(&str, &str); 6] = [
    (
        "init",
        "98653c91da8130472eadfd8f1f43d6c1122b92cea35dd99eb97b4fc7e266984e",
    ),
    (
        "get-default-rules",
        "79ce52ce35c7b1d21a3c9beda60fc2b5ee1edbfda32f33b69d80710bab7e87c4",
    ),
    (
        "get-ui-layout",
        "ede667710e76ddc912372221ba1fffbcf4ee15aa81ee3a5b77b1d172760d92d1",
    ),
    (
        "on-ui-event",
        "4cb7b11d9b1a5124f8db97ebc3e4985103062360edf47da891e18f016c0f99cd",
    ),
    (
        "get-top-tabs",
        "5dbdd83eef659e765dc10c1c975396dbadc603a34ad50ee14987cabdbdaf20bf",
    ),
    (
        "get-metadata",
        "7eaf0705b801e5733a21163dae5c0d2c6e01628c3981d4ebf3f81947424a3188",
    ),
];

const MAX_TYPE_GRAPH_NODES: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentContract {
    pub abi: String,
    pub imports: BTreeSet<String>,
    pub exports: BTreeSet<String>,
}

fn contract_error(classification: &str) -> PluginError {
    PluginError::LoadError(format!(
        "invalid Wirt component contract: component-preflight: {classification}"
    ))
}

fn bounded_name(name: &str) -> String {
    const MAX: usize = 80;
    if name.len() <= MAX {
        return name.to_owned();
    }
    let mut end = MAX;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &name[..end])
}

fn top_level_names(component: &[u8]) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut imports = BTreeSet::new();
    let mut exports = BTreeSet::new();
    let mut depth = 0_usize;
    for payload in Parser::new(0).parse_all(component) {
        match payload.map_err(|_| contract_error("invalid component"))? {
            Payload::Version { encoding, .. } => {
                if depth == 0 && encoding != Encoding::Component {
                    return Err(contract_error("input is not a component"));
                }
                depth += 1;
            }
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::ComponentImportSection(section) if depth == 1 => {
                for import in section {
                    imports.insert(
                        import
                            .map_err(|_| contract_error("invalid component import section"))?
                            .name
                            .name
                            .to_owned(),
                    );
                }
            }
            Payload::ComponentExportSection(section) if depth == 1 => {
                for export in section {
                    exports.insert(
                        export
                            .map_err(|_| contract_error("invalid component export section"))?
                            .name
                            .name
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
    }
    Ok((imports, exports))
}

struct TypeHasher {
    digest: Sha256,
    resources: BTreeMap<ResourceId, u32>,
    nodes: usize,
}

impl TypeHasher {
    fn new() -> Self {
        Self {
            digest: Sha256::new(),
            resources: BTreeMap::new(),
            nodes: 0,
        }
    }

    fn token(&mut self, token: &str) {
        self.digest.update((token.len() as u64).to_le_bytes());
        self.digest.update(token.as_bytes());
    }

    fn node(&mut self, token: &str) -> Result<()> {
        self.nodes += 1;
        if self.nodes > MAX_TYPE_GRAPH_NODES {
            return Err(contract_error(
                "component type graph exceeds the node limit",
            ));
        }
        self.token(token);
        Ok(())
    }

    fn resource(&mut self, resource: ResourceId) {
        let next = self.resources.len() as u32;
        let ordinal = *self.resources.entry(resource).or_insert(next);
        self.token(&format!("resource-{ordinal}"));
    }

    fn entity(&mut self, types: &Types, entity: ComponentEntityType) -> Result<()> {
        self.node("entity")?;
        match entity {
            ComponentEntityType::Func(id) => {
                self.token("func");
                let ty = &types[id];
                self.token(if ty.async_ { "async" } else { "sync" });
                self.token(&ty.params.len().to_string());
                for (name, ty) in ty.params.iter() {
                    self.token(name);
                    self.val(types, *ty)?;
                }
                match ty.result {
                    Some(ty) => {
                        self.token("result");
                        self.val(types, ty)?;
                    }
                    None => self.token("no-result"),
                }
            }
            ComponentEntityType::Value(ty) => {
                self.token("value");
                self.val(types, ty)?;
            }
            ComponentEntityType::Type { referenced, .. } => {
                self.token("type");
                self.any(types, referenced)?;
            }
            ComponentEntityType::Instance(id) => {
                self.token("instance");
                let instance = &types[id];
                let mut exports = instance.exports.iter().collect::<Vec<_>>();
                exports.sort_by(|a, b| a.0.cmp(b.0));
                self.token(&exports.len().to_string());
                for (name, item) in exports {
                    self.token(name);
                    self.entity(types, item.ty)?;
                }
            }
            ComponentEntityType::Component(id) => {
                self.token("component");
                let ty = &types[id];
                let mut imports = ty.imports.iter().collect::<Vec<_>>();
                imports.sort_by(|a, b| a.0.cmp(b.0));
                for (name, item) in imports {
                    self.token("import");
                    self.token(name);
                    self.entity(types, item.ty)?;
                }
                let mut exports = ty.exports.iter().collect::<Vec<_>>();
                exports.sort_by(|a, b| a.0.cmp(b.0));
                for (name, item) in exports {
                    self.token("export");
                    self.token(name);
                    self.entity(types, item.ty)?;
                }
            }
            ComponentEntityType::Module(_) => {
                return Err(contract_error(
                    "core module is not allowed in the public ABI",
                ));
            }
        }
        Ok(())
    }

    fn any(&mut self, types: &Types, ty: ComponentAnyTypeId) -> Result<()> {
        self.node("any")?;
        match ty {
            ComponentAnyTypeId::Resource(id) => self.resource(id.resource()),
            ComponentAnyTypeId::Defined(id) => self.defined(types, &types[id])?,
            ComponentAnyTypeId::Func(id) => self.entity(types, ComponentEntityType::Func(id))?,
            ComponentAnyTypeId::Instance(id) => {
                self.entity(types, ComponentEntityType::Instance(id))?
            }
            ComponentAnyTypeId::Component(id) => {
                self.entity(types, ComponentEntityType::Component(id))?
            }
        }
        Ok(())
    }

    fn val(&mut self, types: &Types, ty: ComponentValType) -> Result<()> {
        self.node("val")?;
        match ty {
            ComponentValType::Primitive(ty) => self.token(&ty.to_string()),
            ComponentValType::Type(id) => self.defined(types, &types[id])?,
        }
        Ok(())
    }

    fn optional_val(&mut self, types: &Types, ty: Option<ComponentValType>) -> Result<()> {
        match ty {
            Some(ty) => {
                self.token("some");
                self.val(types, ty)
            }
            None => {
                self.token("none");
                Ok(())
            }
        }
    }

    fn defined(&mut self, types: &Types, ty: &ComponentDefinedType) -> Result<()> {
        self.node("defined")?;
        match ty {
            ComponentDefinedType::Primitive(ty) => {
                self.token("primitive");
                self.token(&ty.to_string());
            }
            ComponentDefinedType::Record(record) => {
                self.token("record");
                for (name, ty) in &record.fields {
                    self.token(name);
                    self.val(types, *ty)?;
                }
            }
            ComponentDefinedType::Variant(variant) => {
                self.token("variant");
                for (name, case) in &variant.cases {
                    self.token(name);
                    self.optional_val(types, case.ty)?;
                }
            }
            ComponentDefinedType::List(ty) => {
                self.token("list");
                self.val(types, *ty)?;
            }
            ComponentDefinedType::Map(key, value) => {
                self.token("map");
                self.val(types, *key)?;
                self.val(types, *value)?;
            }
            ComponentDefinedType::FixedLengthList(ty, len) => {
                self.token("fixed-list");
                self.token(&len.to_string());
                self.val(types, *ty)?;
            }
            ComponentDefinedType::Tuple(tuple) => {
                self.token("tuple");
                for ty in &tuple.types {
                    self.val(types, *ty)?;
                }
            }
            ComponentDefinedType::Flags(flags) => {
                self.token("flags");
                for name in flags {
                    self.token(name);
                }
            }
            ComponentDefinedType::Enum(cases) => {
                self.token("enum");
                for name in cases {
                    self.token(name);
                }
            }
            ComponentDefinedType::Option(ty) => {
                self.token("option");
                self.val(types, *ty)?;
            }
            ComponentDefinedType::Result { ok, err } => {
                self.token("result");
                self.optional_val(types, *ok)?;
                self.optional_val(types, *err)?;
            }
            ComponentDefinedType::Own(id) => {
                self.token("own");
                self.resource(id.resource());
            }
            ComponentDefinedType::Borrow(id) => {
                self.token("borrow");
                self.resource(id.resource());
            }
            ComponentDefinedType::Future(ty) => {
                self.token("future");
                self.optional_val(types, *ty)?;
            }
            ComponentDefinedType::Stream(ty) => {
                self.token("stream");
                self.optional_val(types, *ty)?;
            }
        }
        Ok(())
    }

    fn finish(self) -> String {
        self.digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn entity_hash(types: &Types, entity: ComponentEntityType) -> Result<String> {
    let mut hasher = TypeHasher::new();
    hasher.entity(types, entity)?;
    Ok(hasher.finish())
}

fn expected_hash<'a>(name: &str, hashes: &'a [(&str, &str)]) -> Option<&'a str> {
    hashes
        .iter()
        .find_map(|(expected, hash)| (*expected == name).then_some(*hash))
}

pub fn inspect_component_contract(component: &[u8]) -> Result<ComponentContract> {
    if component.len() > MAX_PLUGIN_WASM_BYTES {
        return Err(contract_error("component exceeds the 64-MiB limit"));
    }
    let types = Validator::new()
        .validate_all(component)
        .map_err(|_| contract_error("invalid component"))?;
    let (raw_imports, exports) = top_level_names(component)?;
    let mut imports = BTreeSet::new();
    let allowed = WIRT_IMPORTS
        .into_iter()
        .chain(WASI_IMPORTS)
        .collect::<BTreeSet<_>>();

    for name in &raw_imports {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        match item.ty {
            ComponentEntityType::Instance(_) => {
                if !allowed.contains(name.as_str()) {
                    let classification = if name.starts_with("wirt:plugin/") {
                        "unsupported Wirt interface version"
                    } else {
                        "unsupported import"
                    };
                    return Err(contract_error(&format!(
                        "{classification} {:?}",
                        bounded_name(name)
                    )));
                }
                imports.insert(name.clone());
            }
            ComponentEntityType::Type { .. } => {
                let expected = expected_hash(name, &TYPE_IMPORT_HASHES)
                    .ok_or_else(|| contract_error("unsupported public type import"))?;
                if entity_hash(&types, item.ty)? != expected {
                    return Err(contract_error(&format!(
                        "type import mismatch for {:?}",
                        bounded_name(name)
                    )));
                }
            }
            _ => {
                return Err(contract_error(
                    "top-level import is not an interface or type",
                ))
            }
        }
    }
    for name in WIRT_IMPORTS {
        if !imports.contains(name) {
            return Err(contract_error(&format!(
                "missing required Wirt import {name:?}"
            )));
        }
    }

    for name in &imports {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        let expected = expected_hash(name, &IMPORT_TYPE_HASHES)
            .ok_or_else(|| contract_error("unsupported import"))?;
        let actual = entity_hash(&types, item.ty)?;
        if actual != expected {
            return Err(contract_error(&format!(
                "import type mismatch for {:?} ({actual})",
                bounded_name(name)
            )));
        }
    }

    let required_exports = EXPORTS.into_iter().collect::<BTreeSet<_>>();
    if exports.iter().map(String::as_str).collect::<BTreeSet<_>>() != required_exports {
        return Err(contract_error(
            "exports do not match the canonical Wirt world",
        ));
    }
    for name in &exports {
        let item = types
            .component_item_for_export(name)
            .ok_or_else(|| contract_error("validated export is missing type information"))?;
        let expected = expected_hash(name, &EXPORT_TYPE_HASHES)
            .ok_or_else(|| contract_error("unsupported export"))?;
        let actual = entity_hash(&types, item.ty)?;
        if actual != expected {
            return Err(contract_error(&format!(
                "export type mismatch for {:?} ({actual})",
                bounded_name(name)
            )));
        }
    }

    Ok(ComponentContract {
        abi: WIRT_ABI_VERSION.to_string(),
        imports,
        exports,
    })
}
