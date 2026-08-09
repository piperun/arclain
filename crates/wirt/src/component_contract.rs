use crate::{PluginError, Result, MAX_PLUGIN_WASM_BYTES, WIRT_ABI_VERSION};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use wasmparser::component_types::{
    ComponentAnyTypeId, ComponentDefinedType, ComponentDefinedTypeId, ComponentEntityType,
    ComponentFuncTypeId, ComponentInstanceTypeId, ComponentTypeId, ComponentValType,
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

const PUBLIC_TYPE_IMPORTS: [&str; 6] = [
    "plugin-action",
    "plugin-layout",
    "plugin-metadata",
    "plugin-rule-definition",
    "top-tab-config",
    "ui-element",
];

const MAX_TYPE_GRAPH_NODES: usize = 100_000;
const MAX_TYPE_GRAPH_DEPTH: usize = 64;
const EXPECTED_CONTRACT_HASH: &str =
    "542c780fa520e9764f6cbf76db5dc39327cdc878faef455a8daaf7ee5cff495a";

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

enum Work {
    Token(String),
    Entity(ComponentEntityType, usize),
    Any(ComponentAnyTypeId, usize),
    Func(ComponentFuncTypeId, usize),
    Instance(ComponentInstanceTypeId, usize),
    Component(ComponentTypeId, usize),
    Val(ComponentValType, usize),
    OptionalVal(Option<ComponentValType>, usize),
    Defined(ComponentDefinedTypeId, usize),
}

struct TypeHasher<'a> {
    types: &'a Types,
    digest: Sha256,
    identities: BTreeMap<ComponentAnyTypeId, u32>,
    expanded: BTreeSet<ComponentAnyTypeId>,
    work: Vec<Work>,
    scheduled: usize,
}

impl<'a> TypeHasher<'a> {
    fn new(types: &'a Types) -> Self {
        Self {
            types,
            digest: Sha256::new(),
            identities: BTreeMap::new(),
            expanded: BTreeSet::new(),
            work: Vec::new(),
            scheduled: 0,
        }
    }

    fn token(&mut self, token: &str) {
        self.digest.update((token.len() as u64).to_le_bytes());
        self.digest.update(token.as_bytes());
    }

    fn claim(&mut self, count: usize, depth: usize) -> Result<()> {
        if depth > MAX_TYPE_GRAPH_DEPTH {
            return Err(contract_error("type-complexity"));
        }
        self.scheduled = self
            .scheduled
            .checked_add(count)
            .filter(|scheduled| *scheduled <= MAX_TYPE_GRAPH_NODES)
            .ok_or_else(|| contract_error("type-complexity"))?;
        Ok(())
    }

    fn schedule(&mut self, items: Vec<Work>) -> Result<()> {
        let max_depth = items
            .iter()
            .filter_map(|item| match item {
                Work::Token(_) => None,
                Work::Entity(_, depth)
                | Work::Any(_, depth)
                | Work::Func(_, depth)
                | Work::Instance(_, depth)
                | Work::Component(_, depth)
                | Work::Val(_, depth)
                | Work::OptionalVal(_, depth)
                | Work::Defined(_, depth) => Some(*depth),
            })
            .max()
            .unwrap_or(0);
        self.claim(items.len(), max_depth)?;
        self.push_claimed(items);
        Ok(())
    }

    fn push_claimed(&mut self, items: Vec<Work>) {
        self.work.extend(items.into_iter().rev());
    }

    fn canonical_any(
        &mut self,
        mut ty: ComponentAnyTypeId,
        depth: usize,
    ) -> Result<ComponentAnyTypeId> {
        while let Some(peeled) = self.types.peel_alias(ty) {
            self.claim(1, depth)?;
            ty = peeled;
        }
        Ok(ty)
    }

    fn root(&mut self, category: &str, name: &str, entity: ComponentEntityType) -> Result<()> {
        self.token("root");
        self.token(category);
        self.token(name);
        self.schedule(vec![Work::Entity(entity, 0)])?;
        self.drain()
    }

    fn drain(&mut self) -> Result<()> {
        while let Some(work) = self.work.pop() {
            match work {
                Work::Token(token) => self.token(&token),
                Work::Entity(entity, depth) => self.entity(entity, depth)?,
                Work::Any(ty, depth) => self.any(ty, depth)?,
                Work::Func(id, depth) => self.func(id, depth)?,
                Work::Instance(id, depth) => self.instance(id, depth)?,
                Work::Component(id, depth) => self.component(id, depth)?,
                Work::Val(ty, depth) => self.val(ty, depth)?,
                Work::OptionalVal(ty, depth) => self.optional_val(ty, depth)?,
                Work::Defined(id, depth) => self.defined(id, depth)?,
            }
        }
        Ok(())
    }

    fn entity(&mut self, entity: ComponentEntityType, depth: usize) -> Result<()> {
        match entity {
            ComponentEntityType::Func(id) => self.schedule(vec![
                Work::Token("func".into()),
                Work::Any(ComponentAnyTypeId::Func(id), depth + 1),
            ])?,
            ComponentEntityType::Value(ty) => {
                self.schedule(vec![Work::Token("value".into()), Work::Val(ty, depth + 1)])?
            }
            ComponentEntityType::Type {
                referenced,
                created,
            } => self.schedule(vec![
                Work::Token("type".into()),
                Work::Token("referenced".into()),
                Work::Any(referenced, depth + 1),
                Work::Token("created".into()),
                Work::Any(created, depth + 1),
            ])?,
            ComponentEntityType::Instance(id) => self.schedule(vec![
                Work::Token("instance".into()),
                Work::Any(ComponentAnyTypeId::Instance(id), depth + 1),
            ])?,
            ComponentEntityType::Component(id) => self.schedule(vec![
                Work::Token("component".into()),
                Work::Any(ComponentAnyTypeId::Component(id), depth + 1),
            ])?,
            ComponentEntityType::Module(_) => {
                return Err(contract_error(
                    "core module is not allowed in the public ABI",
                ));
            }
        }
        Ok(())
    }

    fn any(&mut self, ty: ComponentAnyTypeId, depth: usize) -> Result<()> {
        let ty = self.canonical_any(ty, depth)?;
        let next = self.identities.len() as u32;
        let ordinal = *self.identities.entry(ty).or_insert(next);
        self.token("identity");
        self.token(&ordinal.to_string());
        if !self.expanded.insert(ty) {
            return Ok(());
        }
        match ty {
            ComponentAnyTypeId::Resource(_) => self.token("resource"),
            ComponentAnyTypeId::Defined(id) => self.schedule(vec![Work::Defined(id, depth + 1)])?,
            ComponentAnyTypeId::Func(id) => self.schedule(vec![Work::Func(id, depth + 1)])?,
            ComponentAnyTypeId::Instance(id) => {
                self.schedule(vec![Work::Instance(id, depth + 1)])?
            }
            ComponentAnyTypeId::Component(id) => {
                self.schedule(vec![Work::Component(id, depth + 1)])?
            }
        }
        Ok(())
    }

    fn func(&mut self, id: ComponentFuncTypeId, depth: usize) -> Result<()> {
        let ty = &self.types[id];
        let count = ty
            .params
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(if ty.result.is_some() { 4 } else { 3 }))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        let mut items = Vec::with_capacity(count);
        items.extend([
            Work::Token(if ty.async_ { "async" } else { "sync" }.into()),
            Work::Token(ty.params.len().to_string()),
        ]);
        for (name, ty) in ty.params.iter() {
            items.push(Work::Token(name.to_string()));
            items.push(Work::Val(*ty, depth + 1));
        }
        match ty.result {
            Some(ty) => {
                items.push(Work::Token("result".into()));
                items.push(Work::Val(ty, depth + 1));
            }
            None => items.push(Work::Token("no-result".into())),
        }
        self.push_claimed(items);
        Ok(())
    }

    fn instance(&mut self, id: ComponentInstanceTypeId, depth: usize) -> Result<()> {
        let instance = &self.types[id];
        let count = instance
            .exports
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        let mut exports = instance.exports.iter().collect::<Vec<_>>();
        exports.sort_by(|a, b| a.0.cmp(b.0));
        let mut items = Vec::with_capacity(count);
        items.push(Work::Token(exports.len().to_string()));
        for (name, item) in exports {
            items.push(Work::Token(name.clone()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        self.push_claimed(items);
        Ok(())
    }

    fn component(&mut self, id: ComponentTypeId, depth: usize) -> Result<()> {
        let ty = &self.types[id];
        let count = ty
            .imports
            .len()
            .checked_add(ty.exports.len())
            .and_then(|count| count.checked_mul(3))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        let mut imports = ty.imports.iter().collect::<Vec<_>>();
        imports.sort_by(|a, b| a.0.cmp(b.0));
        let mut exports = ty.exports.iter().collect::<Vec<_>>();
        exports.sort_by(|a, b| a.0.cmp(b.0));
        let mut items = Vec::with_capacity(count);
        for (name, item) in imports {
            items.push(Work::Token("import".into()));
            items.push(Work::Token(name.clone()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        for (name, item) in exports {
            items.push(Work::Token("export".into()));
            items.push(Work::Token(name.clone()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        self.push_claimed(items);
        Ok(())
    }

    fn val(&mut self, ty: ComponentValType, depth: usize) -> Result<()> {
        match ty {
            ComponentValType::Primitive(ty) => self.token(&ty.to_string()),
            ComponentValType::Type(id) => {
                self.schedule(vec![Work::Any(ComponentAnyTypeId::Defined(id), depth + 1)])?
            }
        }
        Ok(())
    }

    fn optional_val(&mut self, ty: Option<ComponentValType>, depth: usize) -> Result<()> {
        match ty {
            Some(ty) => self.schedule(vec![Work::Token("some".into()), Work::Val(ty, depth + 1)]),
            None => {
                self.token("none");
                Ok(())
            }
        }
    }

    fn defined(&mut self, id: ComponentDefinedTypeId, depth: usize) -> Result<()> {
        match &self.types[id] {
            ComponentDefinedType::Primitive(ty) => {
                self.token("primitive");
                self.token(&ty.to_string());
            }
            ComponentDefinedType::Record(record) => {
                let count = record
                    .fields
                    .len()
                    .checked_mul(2)
                    .and_then(|count| count.checked_add(1))
                    .ok_or_else(|| contract_error("type-complexity"))?;
                self.claim(count, depth + 1)?;
                let mut items = Vec::with_capacity(count);
                items.push(Work::Token("record".into()));
                for (name, ty) in &record.fields {
                    items.push(Work::Token(name.to_string()));
                    items.push(Work::Val(*ty, depth + 1));
                }
                self.push_claimed(items);
            }
            ComponentDefinedType::Variant(variant) => {
                let count = variant
                    .cases
                    .len()
                    .checked_mul(2)
                    .and_then(|count| count.checked_add(1))
                    .ok_or_else(|| contract_error("type-complexity"))?;
                self.claim(count, depth + 1)?;
                let mut items = Vec::with_capacity(count);
                items.push(Work::Token("variant".into()));
                for (name, case) in &variant.cases {
                    items.push(Work::Token(name.to_string()));
                    items.push(Work::OptionalVal(case.ty, depth + 1));
                }
                self.push_claimed(items);
            }
            ComponentDefinedType::List(ty) => {
                self.schedule(vec![Work::Token("list".into()), Work::Val(*ty, depth + 1)])?
            }
            ComponentDefinedType::Map(key, value) => self.schedule(vec![
                Work::Token("map".into()),
                Work::Val(*key, depth + 1),
                Work::Val(*value, depth + 1),
            ])?,
            ComponentDefinedType::FixedLengthList(ty, len) => self.schedule(vec![
                Work::Token("fixed-list".into()),
                Work::Token(len.to_string()),
                Work::Val(*ty, depth + 1),
            ])?,
            ComponentDefinedType::Tuple(tuple) => {
                let count = tuple
                    .types
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| contract_error("type-complexity"))?;
                self.claim(count, depth + 1)?;
                let mut items = Vec::with_capacity(count);
                items.push(Work::Token("tuple".into()));
                for ty in &tuple.types {
                    items.push(Work::Val(*ty, depth + 1));
                }
                self.push_claimed(items);
            }
            ComponentDefinedType::Flags(flags) => {
                self.claim(
                    flags
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| contract_error("type-complexity"))?,
                    depth,
                )?;
                self.token("flags");
                for name in flags {
                    self.token(name);
                }
            }
            ComponentDefinedType::Enum(cases) => {
                self.claim(
                    cases
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| contract_error("type-complexity"))?,
                    depth,
                )?;
                self.token("enum");
                for name in cases {
                    self.token(name);
                }
            }
            ComponentDefinedType::Option(ty) => self.schedule(vec![
                Work::Token("option".into()),
                Work::Val(*ty, depth + 1),
            ])?,
            ComponentDefinedType::Result { ok, err } => self.schedule(vec![
                Work::Token("result".into()),
                Work::OptionalVal(*ok, depth + 1),
                Work::OptionalVal(*err, depth + 1),
            ])?,
            ComponentDefinedType::Own(id) => {
                self.token("own");
                self.schedule(vec![Work::Any(
                    ComponentAnyTypeId::Resource(*id),
                    depth + 1,
                )])?;
            }
            ComponentDefinedType::Borrow(id) => {
                self.token("borrow");
                self.schedule(vec![Work::Any(
                    ComponentAnyTypeId::Resource(*id),
                    depth + 1,
                )])?;
            }
            ComponentDefinedType::Future(ty) => self.schedule(vec![
                Work::Token("future".into()),
                Work::OptionalVal(*ty, depth + 1),
            ])?,
            ComponentDefinedType::Stream(ty) => self.schedule(vec![
                Work::Token("stream".into()),
                Work::OptionalVal(*ty, depth + 1),
            ])?,
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

pub fn inspect_component_contract(component: &[u8]) -> Result<ComponentContract> {
    if component.len() > MAX_PLUGIN_WASM_BYTES {
        return Err(contract_error("component exceeds the 64-MiB limit"));
    }
    let types = Validator::new()
        .validate_all(component)
        .map_err(|_| contract_error("invalid component"))?;
    let (raw_imports, exports) = top_level_names(component)?;
    let runtime_names = WIRT_IMPORTS
        .into_iter()
        .chain(WASI_IMPORTS)
        .collect::<BTreeSet<_>>();
    let public_type_names = PUBLIC_TYPE_IMPORTS.into_iter().collect::<BTreeSet<_>>();
    let mut imports = BTreeSet::new();

    for name in &raw_imports {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        if runtime_names.contains(name.as_str()) {
            match item.ty {
                ComponentEntityType::Instance(_) => {
                    imports.insert(name.clone());
                }
                _ => return Err(contract_error("contract-type-mismatch")),
            }
        } else if public_type_names.contains(name.as_str()) {
            if !matches!(item.ty, ComponentEntityType::Type { .. }) {
                return Err(contract_error("contract-type-mismatch"));
            }
        } else {
            match item.ty {
                ComponentEntityType::Instance(_) => {
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
                ComponentEntityType::Type { .. } => {
                    return Err(contract_error("unsupported public type import"));
                }
                _ => return Err(contract_error("top-level import has an unsupported kind")),
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
    let expected_raw_imports = runtime_names
        .iter()
        .chain(public_type_names.iter())
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if raw_imports != expected_raw_imports {
        return Err(contract_error(
            "imports do not match the canonical Wirt world",
        ));
    }

    let required_exports = EXPORTS.into_iter().collect::<BTreeSet<_>>();
    if exports.iter().map(String::as_str).collect::<BTreeSet<_>>() != required_exports {
        return Err(contract_error(
            "exports do not match the canonical Wirt world",
        ));
    }

    let mut hasher = TypeHasher::new(&types);
    for name in &public_type_names {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        hasher.root("public-type", name, item.ty)?;
    }
    for name in &runtime_names {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        hasher.root("interface", name, item.ty)?;
    }
    for name in &required_exports {
        let item = types
            .component_item_for_export(name)
            .ok_or_else(|| contract_error("validated export is missing type information"))?;
        hasher.root("export", name, item.ty)?;
    }
    let actual = hasher.finish();
    if actual != EXPECTED_CONTRACT_HASH {
        return Err(contract_error(&format!(
            "contract-type-mismatch ({actual})"
        )));
    }

    Ok(ComponentContract {
        abi: WIRT_ABI_VERSION.to_string(),
        imports,
        exports,
    })
}
