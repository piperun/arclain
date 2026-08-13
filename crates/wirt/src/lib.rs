//! Product-neutral plugin kernel.
//!
//! Wirt intentionally contains no manager or product services, so host
//! products can depend on its kernel without inheriting product coupling.

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wirt-sdk/wit/plugin.wit",
        world: "plugin-world",
    });
}

pub mod action_policy;
mod component_contract;
pub mod conversions;
mod error;
mod executor;
pub mod limits;
pub mod loader;
mod manifest;
pub mod model;
mod package;
pub mod rules;
pub mod runtime;
pub mod ui_model;

pub use bindings::{wirt, PluginWorld};
pub use component_contract::{inspect_component_contract, ComponentContract};
pub use error::{PluginError, Result};
pub use executor::{ExecutorRequest, ExecutorResponse, WirtExecutor, MAX_EXECUTOR_MESSAGE_BYTES};
#[doc(hidden)]
pub use executor::{ValidatedExecutorRequest, WirtExecutorBackend};
pub use limits::{
    metadata_value_within_limit, PluginStoreLimiter, StoreQuotaExceeded, StoreQuotaKind,
    MAX_CORE_INSTANCES, MAX_LINEAR_MEMORY_BYTES, MAX_MEMORIES, MAX_PLUGIN_GUEST_DATA_BYTES,
    MAX_PLUGIN_METADATA_BYTES, MAX_TABLES, MAX_TABLE_ELEMENTS,
};
pub use loader::{DiscoveredPlugin, PluginArtifact, PluginLoader, TrustedPluginRoot};
pub use manifest::{
    CapabilitiesConfig, PluginCapability, PluginId, PluginIdentityKey, PluginInfo,
    PluginInfoConfig, PluginManifest, PluginMetadata, RateLimits, WirtConfig,
    REQUEST_FETCH_CAPABILITIES,
};
pub use model::{
    BadgeConfig, ButtonAction, KeyValuePair, PluginAction, PluginExtensionPoint, PluginLayout,
    PluginUiElement, SizeHint, SpacingStep, TextRole, ToastLevel, ToolbarButton, TopTabConfig,
    WarningIcon,
};
pub use package::{
    package_bytes, read_package, read_package_bytes, PackageFingerprint, ValidatedPackage,
    MAX_PLUGIN_MANIFEST_BYTES, MAX_PLUGIN_WASM_BYTES, MAX_WIRT_PACKAGE_BYTES,
};
pub use rules::{
    MoveFileRule, MoveRule, PluginRuleActions, PluginRuleDefinition, PluginRuleTrigger,
};
pub use runtime::{
    sandboxed_wasi_ctx, LoadedComponent, PluginInstance, WasmRuntime, WirtStoreState,
};

pub const WIRT_ABI_VERSION: &str = "0.3.0";
