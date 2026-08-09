//! Arclain-owned runtime context plus compatibility exports from Wirt.

use arclain_core::ArchiveKind;
use serde::{Deserialize, Serialize};

pub use wirt::{
    metadata_value_within_limit, BadgeConfig, ButtonAction, CapabilitiesConfig, KeyValuePair,
    PluginAction, PluginCapability, PluginError, PluginExtensionPoint, PluginId, PluginIdentityKey,
    PluginInfo, PluginInfoConfig, PluginLayout, PluginManifest, PluginMetadata, PluginUiElement,
    RateLimits, Result, ToastLevel, ToolbarButton, TopTabConfig, WarningIcon, WirtConfig,
    MAX_PLUGIN_GUEST_DATA_BYTES, MAX_PLUGIN_METADATA_BYTES, REQUEST_FETCH_CAPABILITIES,
};

#[derive(Clone)]
pub enum PluginEvent {
    OnArchiveOpen {
        path: String,
        kind: ArchiveKind,
        password: Option<String>,
        entries: std::sync::Arc<Vec<String>>,
        archive_session_id: u64,
    },
}

impl std::fmt::Debug for PluginEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnArchiveOpen {
                path,
                kind,
                password,
                entries,
                archive_session_id,
            } => formatter
                .debug_struct("OnArchiveOpen")
                .field("path", path)
                .field("kind", kind)
                .field("password", &password.as_ref().map(|_| "[REDACTED]"))
                .field("entries_len", &entries.len())
                .field("archive_session_id", archive_session_id)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginResponse {
    None,
    Metadata { data: serde_json::Value },
    Error { message: String },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PluginArchiveContextId(u64);

impl PluginArchiveContextId {
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

pub struct PluginArchiveContext {
    pub context_id: PluginArchiveContextId,
    pub path: std::path::PathBuf,
    pub kind: ArchiveKind,
}

pub trait PluginArchiveAccess: Send + Sync {
    fn list_entries(
        &self,
        context_id: PluginArchiveContextId,
    ) -> Result<Vec<arclain_core::ArchiveEntry>>;
    fn read_entry(&self, context_id: PluginArchiveContextId, path: &str) -> Result<Vec<u8>>;
    fn write_metadata(
        &self,
        context_id: PluginArchiveContextId,
        value: serde_json::Value,
    ) -> Result<()>;
}
