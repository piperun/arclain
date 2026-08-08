//! Product-neutral plugin kernel.
//!
//! Wirt intentionally contains no manager or product services, so host
//! products can depend on its kernel without inheriting product coupling.

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/plugin.wit",
        world: "plugin-world",
    });
}

pub use bindings::{wirt, PluginWorld};
