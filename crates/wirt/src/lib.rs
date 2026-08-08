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

pub mod action_policy;
pub mod conversions;
mod error;
pub mod limits;
pub mod loader;
mod manifest;
pub mod model;
pub mod rules;
pub mod runtime;
pub mod ui_model;

pub use bindings::{wirt, PluginWorld};
pub use error::{PluginError, Result};
pub use limits::{
    metadata_value_within_limit, PluginStoreLimiter, StoreQuotaExceeded, StoreQuotaKind,
    MAX_CORE_INSTANCES, MAX_LINEAR_MEMORY_BYTES, MAX_MEMORIES, MAX_PLUGIN_GUEST_DATA_BYTES,
    MAX_PLUGIN_METADATA_BYTES, MAX_TABLES, MAX_TABLE_ELEMENTS,
};
pub use loader::{DiscoveredPlugin, PluginLoader, TrustedPluginRoot};
pub use manifest::{
    CapabilitiesConfig, PluginCapability, PluginId, PluginIdentityKey, PluginInfo,
    PluginInfoConfig, PluginManifest, PluginMetadata, RateLimits, REQUEST_FETCH_CAPABILITIES,
};
pub use model::{
    BadgeConfig, ButtonAction, KeyValuePair, PluginAction, PluginExtensionPoint, PluginLayout,
    PluginUiElement, ToastLevel, ToolbarButton, TopTabConfig, WarningIcon,
};
pub use rules::{
    MoveFileRule, MoveRule, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger,
};
pub use runtime::{LoadedComponent, PluginInstance, WasmRuntime, WirtStoreState};
