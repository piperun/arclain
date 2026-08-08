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
mod limits;
mod manifest;
pub mod model;
pub mod rules;
pub mod ui_model;

pub use bindings::{wirt, PluginWorld};
pub use error::{PluginError, Result};
pub use limits::{
    metadata_value_within_limit, MAX_PLUGIN_GUEST_DATA_BYTES, MAX_PLUGIN_METADATA_BYTES,
};
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
