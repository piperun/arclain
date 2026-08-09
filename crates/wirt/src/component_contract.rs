use crate::{PluginError, PluginWorld, Result, WIRT_ABI_VERSION};
use std::collections::BTreeSet;
use wasmtime::component::{types::ComponentItem, Component, HasSelf, Linker};
use wasmtime::{Config, Engine};

pub(crate) use crate as wirt_crate;
#[path = "runtime/stub_host.rs"]
#[allow(dead_code)]
mod contract_host;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentContract {
    pub abi: String,
    pub imports: BTreeSet<String>,
    pub exports: BTreeSet<String>,
}

fn contract_error(message: impl Into<String>) -> PluginError {
    PluginError::LoadError(format!(
        "invalid Wirt component contract: {}",
        message.into()
    ))
}

pub fn inspect_component_contract(component: &[u8]) -> Result<ComponentContract> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(|error| contract_error(error.to_string()))?;
    let component = Component::from_binary(&engine, component)
        .map_err(|error| contract_error(error.to_string()))?;
    let component_type = component.component_type();

    let imports = component_type
        .imports(&engine)
        .map(|(name, item)| {
            if !matches!(item.ty, ComponentItem::ComponentInstance(_)) {
                return Err(contract_error(format!(
                    "import {name:?} is not an interface"
                )));
            }
            Ok(name.to_owned())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let allowed = WIRT_IMPORTS
        .into_iter()
        .chain(WASI_IMPORTS)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if let Some(name) = imports.difference(&allowed).next() {
        return Err(contract_error(format!("unsupported import {name:?}")));
    }
    for name in WIRT_IMPORTS {
        if !imports.contains(name) {
            return Err(contract_error(format!(
                "missing required Wirt import {name:?}"
            )));
        }
    }

    let exports = component_type
        .exports(&engine)
        .map(|(name, item)| {
            if !matches!(item.ty, ComponentItem::ComponentFunc(_)) {
                return Err(contract_error(format!("export {name:?} is not a function")));
            }
            Ok(name.to_owned())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let required_exports = EXPORTS
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if exports != required_exports {
        return Err(contract_error(
            "exports do not match the canonical Wirt world",
        ));
    }

    let mut linker = Linker::<contract_host::StubHost>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|error| contract_error(error.to_string()))?;
    PluginWorld::add_to_linker::<_, HasSelf<_>>(&mut linker, |host| host)
        .map_err(|error| contract_error(error.to_string()))?;
    let pre = linker
        .instantiate_pre(&component)
        .map_err(|error| contract_error(error.to_string()))?;
    crate::bindings::PluginWorldPre::new(pre).map_err(|error| contract_error(error.to_string()))?;

    Ok(ComponentContract {
        abi: WIRT_ABI_VERSION.to_string(),
        imports,
        exports,
    })
}
