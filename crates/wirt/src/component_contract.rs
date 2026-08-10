use crate::{PluginError, Result, MAX_PLUGIN_WASM_BYTES, WIRT_ABI_VERSION};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use wasmparser::component_types::{
    ComponentAnyTypeId, ComponentDefinedType, ComponentDefinedTypeId, ComponentEntityType,
    ComponentFuncTypeId, ComponentInstanceTypeId, ComponentTypeId, ComponentValType,
};
use wasmparser::types::Types;
use wasmparser::{Encoding, Parser, Payload, PrimitiveValType, Validator};

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

const OPTIONAL_FIXED_WASI_IMPORTS: [&str; 2] = [
    "wasi:clocks/wall-clock@0.2.9",
    "wasi:random/insecure-seed@0.2.9",
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
const MAX_TYPE_GRAPH_TOKEN_BYTES: usize = 64 * 1024;
const EXPECTED_CONTRACT_HASH: &str =
    "a4cd3fed4d07ad7a47ea5ec61a556ac0fc320711a34b130c1b183533b3628fba";
const EXPECTED_FIXED_WASI_CONTRACT_HASH: &str =
    "de3a0cc46ac6621acca0c4efd6f650da656e268401035dc6953f1624dfedf264";

struct CanonicalMember {
    name: &'static str,
    signature: &'static str,
}

struct CanonicalInterface {
    name: &'static str,
    members: &'static [CanonicalMember],
}

include!("wirt_schema.rs");

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

fn claim_token_bytes(total: &mut usize, bytes: usize) -> Result<()> {
    *total = total
        .checked_add(bytes)
        .filter(|bytes| *bytes <= MAX_TYPE_GRAPH_TOKEN_BYTES)
        .ok_or_else(|| contract_error("type-complexity"))?;
    Ok(())
}

fn compare_names(left: &str, right: &str) -> Ordering {
    #[cfg(test)]
    sort_watch::record(left, right);
    left.cmp(right)
}

#[cfg(test)]
mod sort_watch {
    use std::cell::RefCell;

    thread_local! {
        static WATCH: RefCell<Option<(String, usize)>> = const { RefCell::new(None) };
    }

    pub fn start(prefix: &str) {
        WATCH.with(|watch| *watch.borrow_mut() = Some((prefix.to_owned(), 0)));
    }

    pub fn record(left: &str, right: &str) {
        WATCH.with(|watch| {
            if let Some((prefix, comparisons)) = &mut *watch.borrow_mut() {
                if left.starts_with(prefix.as_str()) && right.starts_with(prefix.as_str()) {
                    *comparisons += 1;
                }
            }
        });
    }

    pub fn finish() -> usize {
        WATCH.with(|watch| watch.borrow_mut().take().unwrap().1)
    }
}

fn top_level_names(component: &[u8]) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut imports = BTreeSet::new();
    let mut exports = BTreeSet::new();
    let mut owned_name_bytes = 0;
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
                    let import =
                        import.map_err(|_| contract_error("invalid component import section"))?;
                    claim_token_bytes(&mut owned_name_bytes, import.name.name.len())?;
                    imports.insert(import.name.name.to_owned());
                }
            }
            Payload::ComponentExportSection(section) if depth == 1 => {
                for export in section {
                    let export =
                        export.map_err(|_| contract_error("invalid component export section"))?;
                    claim_token_bytes(&mut owned_name_bytes, export.name.name.len())?;
                    exports.insert(export.name.name.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok((imports, exports))
}

enum Work<'a> {
    Tag(&'static str),
    Name(&'a str),
    ClaimedName(&'a str),
    Number(u64),
    Entity(ComponentEntityType, usize),
    Any(ComponentAnyTypeId, usize),
    FinishAny(ComponentAnyTypeId),
    Func(ComponentFuncTypeId, usize),
    Instance(ComponentInstanceTypeId, usize),
    Component(ComponentTypeId, usize),
    Val(ComponentValType, usize),
    OptionalVal(Option<ComponentValType>, usize),
    Defined(ComponentDefinedTypeId, usize),
}

struct TypeHasher<'a> {
    types: &'a Types,
    frames: Vec<Sha256>,
    resources: BTreeMap<ComponentAnyTypeId, u32>,
    structural: BTreeMap<ComponentAnyTypeId, [u8; 32]>,
    active: BTreeMap<ComponentAnyTypeId, usize>,
    work: Vec<Work<'a>>,
    scheduled: usize,
    token_bytes: usize,
}

impl<'a> TypeHasher<'a> {
    fn new(types: &'a Types) -> Self {
        Self {
            types,
            frames: vec![Sha256::new()],
            resources: BTreeMap::new(),
            structural: BTreeMap::new(),
            active: BTreeMap::new(),
            work: Vec::new(),
            scheduled: 0,
            token_bytes: 0,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        let digest = self.frames.last_mut().expect("the world frame is present");
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }

    fn tag(&mut self, tag: &'static str) {
        self.update(tag.as_bytes());
    }

    fn name(&mut self, name: &str) -> Result<()> {
        claim_token_bytes(&mut self.token_bytes, name.len())?;
        self.update(name.as_bytes());
        Ok(())
    }

    fn claim_names<'b>(&mut self, names: impl IntoIterator<Item = &'b str>) -> Result<()> {
        let bytes = names
            .into_iter()
            .try_fold(0_usize, |bytes, name| bytes.checked_add(name.len()))
            .ok_or_else(|| contract_error("type-complexity"))?;
        claim_token_bytes(&mut self.token_bytes, bytes)
    }

    fn number(&mut self, number: u64) {
        self.tag("number");
        self.update(&number.to_le_bytes());
    }

    fn primitive(&mut self, ty: PrimitiveValType) {
        self.tag(match ty {
            PrimitiveValType::Bool => "bool",
            PrimitiveValType::S8 => "s8",
            PrimitiveValType::U8 => "u8",
            PrimitiveValType::S16 => "s16",
            PrimitiveValType::U16 => "u16",
            PrimitiveValType::S32 => "s32",
            PrimitiveValType::U32 => "u32",
            PrimitiveValType::S64 => "s64",
            PrimitiveValType::U64 => "u64",
            PrimitiveValType::F32 => "f32",
            PrimitiveValType::F64 => "f64",
            PrimitiveValType::Char => "char",
            PrimitiveValType::String => "string",
            PrimitiveValType::ErrorContext => "error-context",
        });
    }

    fn structural_digest(&mut self, digest: &[u8; 32]) {
        self.tag("structural-digest");
        self.update(digest);
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

    fn schedule(&mut self, items: Vec<Work<'a>>) -> Result<()> {
        let max_depth = items
            .iter()
            .filter_map(|item| match item {
                Work::Tag(_)
                | Work::Name(_)
                | Work::ClaimedName(_)
                | Work::Number(_)
                | Work::FinishAny(_) => None,
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

    fn push_claimed(&mut self, items: Vec<Work<'a>>) {
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

    fn root(
        &mut self,
        category: &'static str,
        name: &'a str,
        entity: ComponentEntityType,
    ) -> Result<()> {
        self.tag("root");
        self.tag(category);
        self.name(name)?;
        self.schedule(vec![Work::Entity(entity, 0)])?;
        self.drain()
    }

    fn drain(&mut self) -> Result<()> {
        while let Some(work) = self.work.pop() {
            match work {
                Work::Tag(tag) => self.tag(tag),
                Work::Name(name) => self.name(name)?,
                Work::ClaimedName(name) => self.update(name.as_bytes()),
                Work::Number(number) => self.number(number),
                Work::Entity(entity, depth) => self.entity(entity, depth)?,
                Work::Any(ty, depth) => self.any(ty, depth)?,
                Work::FinishAny(ty) => self.finish_any(ty)?,
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
                Work::Tag("func"),
                Work::Any(ComponentAnyTypeId::Func(id), depth + 1),
            ])?,
            ComponentEntityType::Value(ty) => {
                self.schedule(vec![Work::Tag("value"), Work::Val(ty, depth + 1)])?
            }
            ComponentEntityType::Type {
                referenced,
                created,
            } => self.schedule(vec![
                Work::Tag("type"),
                Work::Tag("referenced"),
                Work::Any(referenced, depth + 1),
                Work::Tag("created"),
                Work::Any(created, depth + 1),
            ])?,
            ComponentEntityType::Instance(id) => self.schedule(vec![
                Work::Tag("instance"),
                Work::Any(ComponentAnyTypeId::Instance(id), depth + 1),
            ])?,
            ComponentEntityType::Component(id) => self.schedule(vec![
                Work::Tag("component"),
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
        if matches!(ty, ComponentAnyTypeId::Resource(_)) {
            let next = self.resources.len() as u32;
            let ordinal = *self.resources.entry(ty).or_insert(next);
            self.tag("resource");
            self.number(ordinal.into());
            return Ok(());
        }
        if let Some(digest) = self.structural.get(&ty).copied() {
            self.structural_digest(&digest);
            return Ok(());
        }
        if let Some(frame) = self.active.get(&ty).copied() {
            self.tag("active-cycle");
            self.number((self.frames.len() - frame) as u64);
            return Ok(());
        }

        self.active.insert(ty, self.frames.len());
        self.frames.push(Sha256::new());
        let mut items = match ty {
            ComponentAnyTypeId::Resource(_) => unreachable!(),
            ComponentAnyTypeId::Defined(id) => {
                vec![Work::Tag("defined"), Work::Defined(id, depth + 1)]
            }
            ComponentAnyTypeId::Func(id) => {
                vec![Work::Tag("func"), Work::Func(id, depth + 1)]
            }
            ComponentAnyTypeId::Instance(id) => {
                vec![Work::Tag("instance"), Work::Instance(id, depth + 1)]
            }
            ComponentAnyTypeId::Component(id) => {
                vec![Work::Tag("component"), Work::Component(id, depth + 1)]
            }
        };
        items.push(Work::FinishAny(ty));
        self.schedule(items)?;
        Ok(())
    }

    fn finish_any(&mut self, ty: ComponentAnyTypeId) -> Result<()> {
        let digest = self
            .frames
            .pop()
            .filter(|_| !self.frames.is_empty())
            .ok_or_else(|| contract_error("type-complexity"))?
            .finalize()
            .into();
        self.active.remove(&ty);
        self.structural.insert(ty, digest);
        self.structural_digest(&digest);
        Ok(())
    }

    fn func(&mut self, id: ComponentFuncTypeId, depth: usize) -> Result<()> {
        let ty: &'a _ = &self.types[id];
        let count = ty
            .params
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(if ty.result.is_some() { 4 } else { 3 }))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        let mut items = Vec::with_capacity(count);
        items.extend([
            Work::Tag(if ty.async_ { "async" } else { "sync" }),
            Work::Number(ty.params.len() as u64),
        ]);
        for (name, ty) in ty.params.iter() {
            items.push(Work::Name(name.as_str()));
            items.push(Work::Val(*ty, depth + 1));
        }
        match ty.result {
            Some(ty) => {
                items.push(Work::Tag("result"));
                items.push(Work::Val(ty, depth + 1));
            }
            None => items.push(Work::Tag("no-result")),
        }
        self.push_claimed(items);
        Ok(())
    }

    fn instance(&mut self, id: ComponentInstanceTypeId, depth: usize) -> Result<()> {
        let instance: &'a _ = &self.types[id];
        let count = instance
            .exports
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        self.claim_names(instance.exports.keys().map(String::as_str))?;
        let mut exports = instance.exports.iter().collect::<Vec<_>>();
        exports.sort_by(|a, b| compare_names(a.0.as_str(), b.0.as_str()));
        let mut items = Vec::with_capacity(count);
        items.push(Work::Number(exports.len() as u64));
        for (name, item) in exports {
            items.push(Work::ClaimedName(name.as_str()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        self.push_claimed(items);
        Ok(())
    }

    fn component(&mut self, id: ComponentTypeId, depth: usize) -> Result<()> {
        let ty: &'a _ = &self.types[id];
        let count = ty
            .imports
            .len()
            .checked_add(ty.exports.len())
            .and_then(|count| count.checked_mul(3))
            .ok_or_else(|| contract_error("type-complexity"))?;
        self.claim(count, depth + 1)?;
        self.claim_names(
            ty.imports
                .keys()
                .chain(ty.exports.keys())
                .map(String::as_str),
        )?;
        let mut imports = ty.imports.iter().collect::<Vec<_>>();
        imports.sort_by(|a, b| compare_names(a.0.as_str(), b.0.as_str()));
        let mut exports = ty.exports.iter().collect::<Vec<_>>();
        exports.sort_by(|a, b| compare_names(a.0.as_str(), b.0.as_str()));
        let mut items = Vec::with_capacity(count);
        for (name, item) in imports {
            items.push(Work::Tag("import"));
            items.push(Work::ClaimedName(name.as_str()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        for (name, item) in exports {
            items.push(Work::Tag("export"));
            items.push(Work::ClaimedName(name.as_str()));
            items.push(Work::Entity(item.ty, depth + 1));
        }
        self.push_claimed(items);
        Ok(())
    }

    fn val(&mut self, ty: ComponentValType, depth: usize) -> Result<()> {
        match ty {
            ComponentValType::Primitive(ty) => self.primitive(ty),
            ComponentValType::Type(id) => {
                self.schedule(vec![Work::Any(ComponentAnyTypeId::Defined(id), depth + 1)])?
            }
        }
        Ok(())
    }

    fn optional_val(&mut self, ty: Option<ComponentValType>, depth: usize) -> Result<()> {
        match ty {
            Some(ty) => self.schedule(vec![Work::Tag("some"), Work::Val(ty, depth + 1)]),
            None => {
                self.tag("none");
                Ok(())
            }
        }
    }

    fn defined(&mut self, id: ComponentDefinedTypeId, depth: usize) -> Result<()> {
        let defined: &'a ComponentDefinedType = &self.types[id];
        match defined {
            ComponentDefinedType::Primitive(ty) => {
                self.tag("primitive");
                self.primitive(*ty);
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
                items.push(Work::Tag("record"));
                for (name, ty) in &record.fields {
                    items.push(Work::Name(name.as_str()));
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
                items.push(Work::Tag("variant"));
                for (name, case) in &variant.cases {
                    items.push(Work::Name(name.as_str()));
                    items.push(Work::OptionalVal(case.ty, depth + 1));
                }
                self.push_claimed(items);
            }
            ComponentDefinedType::List(ty) => {
                self.schedule(vec![Work::Tag("list"), Work::Val(*ty, depth + 1)])?
            }
            ComponentDefinedType::Map(key, value) => self.schedule(vec![
                Work::Tag("map"),
                Work::Val(*key, depth + 1),
                Work::Val(*value, depth + 1),
            ])?,
            ComponentDefinedType::FixedLengthList(ty, len) => self.schedule(vec![
                Work::Tag("fixed-list"),
                Work::Number((*len).into()),
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
                items.push(Work::Tag("tuple"));
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
                self.tag("flags");
                for name in flags {
                    self.name(name)?;
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
                self.tag("enum");
                for name in cases {
                    self.name(name)?;
                }
            }
            ComponentDefinedType::Option(ty) => {
                self.schedule(vec![Work::Tag("option"), Work::Val(*ty, depth + 1)])?
            }
            ComponentDefinedType::Result { ok, err } => self.schedule(vec![
                Work::Tag("result"),
                Work::OptionalVal(*ok, depth + 1),
                Work::OptionalVal(*err, depth + 1),
            ])?,
            ComponentDefinedType::Own(id) => {
                self.tag("own");
                self.schedule(vec![Work::Any(
                    ComponentAnyTypeId::Resource(*id),
                    depth + 1,
                )])?;
            }
            ComponentDefinedType::Borrow(id) => {
                self.tag("borrow");
                self.schedule(vec![Work::Any(
                    ComponentAnyTypeId::Resource(*id),
                    depth + 1,
                )])?;
            }
            ComponentDefinedType::Future(ty) => {
                self.schedule(vec![Work::Tag("future"), Work::OptionalVal(*ty, depth + 1)])?
            }
            ComponentDefinedType::Stream(ty) => {
                self.schedule(vec![Work::Tag("stream"), Work::OptionalVal(*ty, depth + 1)])?
            }
        }
        Ok(())
    }

    fn finish(mut self) -> String {
        debug_assert_eq!(self.frames.len(), 1);
        self.frames
            .pop()
            .expect("the world frame is present")
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

struct InterfaceSignature<'a> {
    types: &'a Types,
    resources: BTreeMap<ComponentAnyTypeId, &'a str>,
    active: BTreeSet<ComponentAnyTypeId>,
    nodes: usize,
    token_bytes: usize,
}

impl<'a> InterfaceSignature<'a> {
    fn new(types: &'a Types, instance: ComponentInstanceTypeId) -> Result<Self> {
        let mut renderer = Self {
            types,
            resources: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
            token_bytes: 0,
        };
        let instance = &types[instance];
        for (name, item) in &instance.exports {
            renderer.claim(name.len(), 0)?;
            if let ComponentEntityType::Type {
                referenced,
                created,
            } = item.ty
            {
                for id in [referenced, created] {
                    let id = renderer.canonical_any(id, 0)?;
                    if matches!(id, ComponentAnyTypeId::Resource(_)) {
                        match renderer.resources.insert(id, name.as_str()) {
                            Some(previous) if previous != name => {
                                return Err(contract_error("contract-type-mismatch"));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(renderer)
    }

    fn claim(&mut self, bytes: usize, depth: usize) -> Result<()> {
        if depth > MAX_TYPE_GRAPH_DEPTH {
            return Err(contract_error("type-complexity"));
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .filter(|nodes| *nodes <= MAX_TYPE_GRAPH_NODES)
            .ok_or_else(|| contract_error("type-complexity"))?;
        claim_token_bytes(&mut self.token_bytes, bytes)
    }

    fn push(&mut self, output: &mut String, value: &str, depth: usize) -> Result<()> {
        self.claim(value.len(), depth)?;
        output.push_str(value);
        Ok(())
    }

    fn canonical_any(
        &mut self,
        mut id: ComponentAnyTypeId,
        depth: usize,
    ) -> Result<ComponentAnyTypeId> {
        while let Some(peeled) = self.types.peel_alias(id) {
            self.claim(0, depth)?;
            id = peeled;
        }
        Ok(id)
    }

    fn entity(&mut self, entity: ComponentEntityType) -> Result<String> {
        let mut output = String::new();
        match entity {
            ComponentEntityType::Func(id) => self.func(&mut output, id, 0)?,
            ComponentEntityType::Type { created, .. } => {
                self.push(&mut output, "type:", 0)?;
                let id = self.canonical_any(created, 0)?;
                match id {
                    ComponentAnyTypeId::Resource(_) => {
                        self.push(&mut output, "resource", 1)?;
                    }
                    ComponentAnyTypeId::Defined(id) => {
                        self.defined(&mut output, id, 1)?;
                    }
                    _ => return Err(contract_error("contract-type-mismatch")),
                }
            }
            _ => return Err(contract_error("contract-type-mismatch")),
        }
        Ok(output)
    }

    fn primitive(&mut self, output: &mut String, ty: PrimitiveValType, depth: usize) -> Result<()> {
        self.push(
            output,
            match ty {
                PrimitiveValType::Bool => "bool",
                PrimitiveValType::S8 => "s8",
                PrimitiveValType::U8 => "u8",
                PrimitiveValType::S16 => "s16",
                PrimitiveValType::U16 => "u16",
                PrimitiveValType::S32 => "s32",
                PrimitiveValType::U32 => "u32",
                PrimitiveValType::S64 => "s64",
                PrimitiveValType::U64 => "u64",
                PrimitiveValType::F32 => "f32",
                PrimitiveValType::F64 => "f64",
                PrimitiveValType::Char => "char",
                PrimitiveValType::String => "string",
                PrimitiveValType::ErrorContext => "error-context",
            },
            depth,
        )
    }

    fn val(&mut self, output: &mut String, ty: ComponentValType, depth: usize) -> Result<()> {
        match ty {
            ComponentValType::Primitive(ty) => self.primitive(output, ty, depth),
            ComponentValType::Type(id) => self.defined(output, id, depth),
        }
    }

    fn optional_val(
        &mut self,
        output: &mut String,
        ty: Option<ComponentValType>,
        depth: usize,
    ) -> Result<()> {
        match ty {
            Some(ty) => self.val(output, ty, depth),
            None => self.push(output, "none", depth),
        }
    }

    fn func(&mut self, output: &mut String, id: ComponentFuncTypeId, depth: usize) -> Result<()> {
        let function = &self.types[id];
        self.push(
            output,
            if function.async_ {
                "func:async("
            } else {
                "func:sync("
            },
            depth,
        )?;
        for (index, (name, ty)) in function.params.iter().enumerate() {
            if index > 0 {
                self.push(output, ",", depth)?;
            }
            self.push(output, name, depth)?;
            self.push(output, "=", depth)?;
            self.val(output, *ty, depth + 1)?;
        }
        self.push(output, ")->", depth)?;
        self.optional_val(output, function.result, depth + 1)
    }

    fn any(&mut self, output: &mut String, id: ComponentAnyTypeId, depth: usize) -> Result<()> {
        let id = self.canonical_any(id, depth)?;
        match id {
            ComponentAnyTypeId::Resource(_) => {
                let name = self
                    .resources
                    .get(&id)
                    .copied()
                    .ok_or_else(|| contract_error("contract-type-mismatch"))?;
                self.push(output, name, depth)
            }
            ComponentAnyTypeId::Defined(id) => self.defined(output, id, depth),
            _ => Err(contract_error("contract-type-mismatch")),
        }
    }

    fn defined(
        &mut self,
        output: &mut String,
        id: ComponentDefinedTypeId,
        depth: usize,
    ) -> Result<()> {
        let any = self.canonical_any(ComponentAnyTypeId::Defined(id), depth)?;
        let ComponentAnyTypeId::Defined(id) = any else {
            return self.any(output, any, depth);
        };
        if !self.active.insert(any) {
            return Err(contract_error("type-complexity"));
        }
        self.claim(0, depth)?;
        let result = match &self.types[id] {
            ComponentDefinedType::Primitive(ty) => self.primitive(output, *ty, depth),
            ComponentDefinedType::Record(record) => {
                self.push(output, "record(", depth)?;
                for (index, (name, ty)) in record.fields.iter().enumerate() {
                    if index > 0 {
                        self.push(output, ",", depth)?;
                    }
                    self.push(output, name, depth)?;
                    self.push(output, "=", depth)?;
                    self.val(output, *ty, depth + 1)?;
                }
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Variant(variant) => {
                self.push(output, "variant(", depth)?;
                for (index, (name, case)) in variant.cases.iter().enumerate() {
                    if index > 0 {
                        self.push(output, ",", depth)?;
                    }
                    self.push(output, name, depth)?;
                    self.push(output, "=", depth)?;
                    self.optional_val(output, case.ty, depth + 1)?;
                }
                self.push(output, ")", depth)
            }
            ComponentDefinedType::List(ty) => {
                self.push(output, "list(", depth)?;
                self.val(output, *ty, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Map(key, value) => {
                self.push(output, "map(", depth)?;
                self.val(output, *key, depth + 1)?;
                self.push(output, ",", depth)?;
                self.val(output, *value, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::FixedLengthList(ty, length) => {
                self.push(output, "fixed-list(", depth)?;
                write!(output, "{length}").unwrap();
                self.claim(length.to_string().len(), depth)?;
                self.push(output, ",", depth)?;
                self.val(output, *ty, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Tuple(tuple) => {
                self.push(output, "tuple(", depth)?;
                for (index, ty) in tuple.types.iter().enumerate() {
                    if index > 0 {
                        self.push(output, ",", depth)?;
                    }
                    self.val(output, *ty, depth + 1)?;
                }
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Flags(flags) => {
                self.push(output, "flags(", depth)?;
                for (index, name) in flags.iter().enumerate() {
                    if index > 0 {
                        self.push(output, ",", depth)?;
                    }
                    self.push(output, name, depth)?;
                }
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Enum(cases) => {
                self.push(output, "enum(", depth)?;
                for (index, name) in cases.iter().enumerate() {
                    if index > 0 {
                        self.push(output, ",", depth)?;
                    }
                    self.push(output, name, depth)?;
                }
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Option(ty) => {
                self.push(output, "option(", depth)?;
                self.val(output, *ty, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Result { ok, err } => {
                self.push(output, "result(", depth)?;
                self.optional_val(output, *ok, depth + 1)?;
                self.push(output, ",", depth)?;
                self.optional_val(output, *err, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Own(resource) => {
                self.push(output, "own(", depth)?;
                self.any(output, ComponentAnyTypeId::Resource(*resource), depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Borrow(resource) => {
                self.push(output, "borrow(", depth)?;
                self.any(output, ComponentAnyTypeId::Resource(*resource), depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Future(ty) => {
                self.push(output, "future(", depth)?;
                self.optional_val(output, *ty, depth + 1)?;
                self.push(output, ")", depth)
            }
            ComponentDefinedType::Stream(ty) => {
                self.push(output, "stream(", depth)?;
                self.optional_val(output, *ty, depth + 1)?;
                self.push(output, ")", depth)
            }
        };
        self.active.remove(&any);
        result
    }
}

fn validate_wirt_interface(
    types: &Types,
    name: &str,
    instance: ComponentInstanceTypeId,
) -> Result<()> {
    let schema = CANONICAL_WIRT_INTERFACES
        .binary_search_by_key(&name, |schema| schema.name)
        .ok()
        .map(|index| &CANONICAL_WIRT_INTERFACES[index])
        .ok_or_else(|| contract_error("contract-type-mismatch"))?;
    let mut renderer = InterfaceSignature::new(types, instance)?;
    for (member_name, member) in &types[instance].exports {
        let expected = schema
            .members
            .binary_search_by_key(&member_name.as_str(), |member| member.name)
            .ok()
            .map(|index| schema.members[index].signature)
            .ok_or_else(|| contract_error("contract-type-mismatch"))?;
        if renderer.entity(member.ty)? != expected {
            return Err(contract_error("contract-type-mismatch"));
        }
    }
    Ok(())
}

fn fixed_wasi_profile(
    raw_imports: &BTreeSet<String>,
    base_imports: &BTreeSet<String>,
) -> Result<bool> {
    if raw_imports == base_imports {
        return Ok(false);
    }
    let fixed_imports = base_imports
        .iter()
        .cloned()
        .chain(OPTIONAL_FIXED_WASI_IMPORTS.into_iter().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    if raw_imports == &fixed_imports {
        return Ok(true);
    }
    Err(contract_error(
        "imports do not match the canonical Wirt world",
    ))
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
        .chain(OPTIONAL_FIXED_WASI_IMPORTS)
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
    let expected_raw_imports = WIRT_IMPORTS
        .into_iter()
        .chain(WASI_IMPORTS)
        .chain(public_type_names.iter().copied())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let uses_fixed_wasi = fixed_wasi_profile(&raw_imports, &expected_raw_imports)?;

    let required_exports = EXPORTS.into_iter().collect::<BTreeSet<_>>();
    if exports.iter().map(String::as_str).collect::<BTreeSet<_>>() != required_exports {
        return Err(contract_error(
            "exports do not match the canonical Wirt world",
        ));
    }

    for name in WIRT_IMPORTS {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        let ComponentEntityType::Instance(instance) = item.ty else {
            return Err(contract_error("contract-type-mismatch"));
        };
        validate_wirt_interface(&types, name, instance)?;
    }

    let mut hasher = TypeHasher::new(&types);
    for name in &public_type_names {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        hasher.root("public-type", name, item.ty)?;
    }
    for name in WASI_IMPORTS {
        let item = types
            .component_item_for_import(name)
            .ok_or_else(|| contract_error("validated import is missing type information"))?;
        hasher.root("interface", name, item.ty)?;
    }
    if uses_fixed_wasi {
        for name in OPTIONAL_FIXED_WASI_IMPORTS {
            let item = types
                .component_item_for_import(name)
                .ok_or_else(|| contract_error("validated import is missing type information"))?;
            hasher.root("interface", name, item.ty)?;
        }
    }
    for name in &required_exports {
        let item = types
            .component_item_for_export(name)
            .ok_or_else(|| contract_error("validated export is missing type information"))?;
        hasher.root("export", name, item.ty)?;
    }
    let actual = hasher.finish();
    let expected = if uses_fixed_wasi {
        EXPECTED_FIXED_WASI_CONTRACT_HASH
    } else {
        EXPECTED_CONTRACT_HASH
    };
    if actual != expected {
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

#[cfg(test)]
mod tests {
    use super::{fixed_wasi_profile, inspect_component_contract, sort_watch};
    use wasm_encoder::reencode::ReencodeComponent;

    const UI_DEMO_COMPONENT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/ui-demo/ui-demo.wasm"
    ));
    const WATCHED_PREFIX: &str = "many-common-prefix-segments-for-sort-review";

    #[test]
    fn fixed_wasi_profile_accepts_only_the_complete_fixed_pair() {
        let base = ["wasi:io/poll@0.2.9"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(!fixed_wasi_profile(&base, &base).unwrap());

        let mut fixed = base.clone();
        fixed.insert("wasi:clocks/wall-clock@0.2.9".to_string());
        fixed.insert("wasi:random/insecure-seed@0.2.9".to_string());
        assert!(fixed_wasi_profile(&fixed, &base).unwrap());

        fixed.remove("wasi:random/insecure-seed@0.2.9");
        assert!(fixed_wasi_profile(&fixed, &base).is_err());
    }

    struct ManyCommonPrefixNames;

    impl wasm_encoder::reencode::Reencode for ManyCommonPrefixNames {
        type Error = std::convert::Infallible;
    }

    impl ReencodeComponent for ManyCommonPrefixNames {
        fn component_instance_type(
            &mut self,
            declarations: Box<[wasmparser::InstanceTypeDeclaration<'_>]>,
        ) -> Result<wasm_encoder::InstanceType, wasm_encoder::reencode::Error<Self::Error>>
        {
            use wasm_encoder::{ComponentTypeRef, InstanceType, PrimitiveValType, TypeBounds};
            use wasmparser::InstanceTypeDeclaration;

            let selected = declarations.iter().any(|declaration| {
                matches!(
                    declaration,
                    InstanceTypeDeclaration::Export { name, .. } if name.name == "log"
                )
            });
            let mut instance = InstanceType::new();
            for declaration in declarations {
                self.parse_component_instance_type_declaration(&mut instance, declaration)?;
            }
            if selected {
                for index in 0..2_000 {
                    let ty = instance.type_count();
                    instance
                        .ty()
                        .defined_type()
                        .primitive(PrimitiveValType::U32);
                    instance.export(
                        &format!("{WATCHED_PREFIX}-{index:04}"),
                        ComponentTypeRef::Type(TypeBounds::Eq(ty)),
                    );
                }
            }
            Ok(instance)
        }
    }

    fn component_with_many_common_prefix_names() -> Vec<u8> {
        let mut mutation = ManyCommonPrefixNames;
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
    fn oversized_nested_name_set_is_rejected_before_sort_comparisons() {
        let component = component_with_many_common_prefix_names();
        wasmparser::Validator::new()
            .validate_all(&component)
            .unwrap();

        sort_watch::start(WATCHED_PREFIX);
        let error = inspect_component_contract(&component)
            .unwrap_err()
            .to_string();
        let comparisons = sort_watch::finish();

        assert!(
            error.contains("component-preflight: type-complexity"),
            "unexpected classification: {error}"
        );
        assert_eq!(
            comparisons, 0,
            "oversized nested names reached sorting before their byte budget"
        );
    }
}
