use crate::{PluginError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PluginId(String);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PluginIdentityKey(String);

impl PluginId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let base_name = value
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(base_name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base_name.len() == 4
                && (base_name.starts_with("COM") || base_name.starts_with("LPT"))
                && matches!(base_name.as_bytes()[3], b'1'..=b'9'));
        let portable = !value.is_empty()
            && value.len() <= 64
            && value != "."
            && value != ".."
            && !reserved
            && !value.ends_with('.')
            && !value.ends_with(' ')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));

        portable.then_some(Self(value)).ok_or_else(|| {
            PluginError::InvalidManifest(
                "Plugin ID must be at most 64 bytes and one ASCII filename component using [A-Za-z0-9_-], and must not be a Windows reserved name".into(),
            )
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join_under(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }

    pub fn identity_key(&self) -> PluginIdentityKey {
        PluginIdentityKey(self.0.to_ascii_lowercase())
    }
}

impl PluginIdentityKey {
    pub fn parse(value: &str) -> Result<Self> {
        PluginId::parse(value.to_owned()).map(|plugin_id| plugin_id.identity_key())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join_under(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }
}

impl std::fmt::Display for PluginIdentityKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginCapability {
    FileRead,
    FileWrite,
    Network,
    ArchiveMetadataRead,
    ArchiveMetadataWrite,
    ArchiveModify,
}

pub const REQUEST_FETCH_CAPABILITIES: [PluginCapability; 2] = [
    PluginCapability::Network,
    PluginCapability::ArchiveMetadataWrite,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WirtConfig {
    pub abi: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub wirt: WirtConfig,
    pub plugin: PluginInfoConfig,
    pub capabilities: CapabilitiesConfig,
    #[serde(default)]
    pub rate_limits: RateLimits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfoConfig {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CapabilitiesConfig {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub network_domains: Vec<String>,
    #[serde(default)]
    pub archive_metadata_read: bool,
    #[serde(default)]
    pub archive_metadata_write: bool,
    #[serde(default)]
    pub archive_modify: bool,
    #[serde(default)]
    pub file_read: bool,
    #[serde(default)]
    pub file_write: bool,
}

impl CapabilitiesConfig {
    pub fn to_capabilities(&self) -> Vec<PluginCapability> {
        let mut caps = Vec::new();
        if self.network {
            caps.push(PluginCapability::Network);
        }
        if self.archive_metadata_read {
            caps.push(PluginCapability::ArchiveMetadataRead);
        }
        if self.archive_metadata_write {
            caps.push(PluginCapability::ArchiveMetadataWrite);
        }
        if self.archive_modify {
            caps.push(PluginCapability::ArchiveModify);
        }
        if self.file_read {
            caps.push(PluginCapability::FileRead);
        }
        if self.file_write {
            caps.push(PluginCapability::FileWrite);
        }
        caps
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimits {
    #[serde(default = "default_http_requests_per_minute")]
    pub http_requests_per_minute: u32,
}

fn default_http_requests_per_minute() -> u32 {
    10
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            http_requests_per_minute: default_http_requests_per_minute(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginInfo {
    pub metadata: PluginMetadata,
    pub capabilities: Vec<PluginCapability>,
    pub manifest_path: PathBuf,
    pub wasm_path: PathBuf,
}
