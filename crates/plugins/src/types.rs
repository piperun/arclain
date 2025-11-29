//! Core types for the plugin system

use arclain_core::{ArchiveEntry, ArchiveKind};
use serde::{Deserialize, Serialize};

/// Plugin metadata describing the plugin's identity and capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

/// Capabilities that a plugin can request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginCapability {
    /// Read files from archives
    FileRead,
    /// Write files to archives
    FileWrite,
    /// Make network requests
    Network,
    /// Read archive metadata
    ArchiveMetadataRead,
    /// Write archive metadata
    ArchiveMetadataWrite,
    /// Modify archive structure
    ArchiveModify,
}

/// Events that plugins can subscribe to
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginEvent {
    /// Archive was opened
    OnArchiveOpen {
        path: String,
        kind: ArchiveKind,
    },
    /// Archive was closed
    OnArchiveClose {
        path: String,
    },
    /// Archive contents were listed
    OnArchiveList {
        path: String,
        entries: Vec<ArchiveEntry>,
    },
    /// File was extracted from archive
    OnFileExtract {
        archive: String,
        file_path: String,
    },
    /// File was opened from archive
    OnFileOpen {
        archive: String,
        file_path: String,
    },
    /// File was added to archive
    OnFileAdd {
        archive: String,
        file_path: String,
    },
    /// File was deleted from archive
    OnFileDelete {
        archive: String,
        file_path: String,
    },
    /// Metadata display requested
    OnMetadataDisplay {
        archive: String,
    },
}

/// Response from a plugin after handling an event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginResponse {
    /// No response needed
    None,
    /// Plugin provided metadata
    Metadata {
        data: serde_json::Value,
    },
    /// Plugin encountered an error
    Error {
        message: String,
    },
}

/// Plugin manifest loaded from plugin.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginInfoConfig,
    pub capabilities: CapabilitiesConfig,
    #[serde(default)]
    pub rate_limits: RateLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfoConfig {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilitiesConfig {
    #[serde(default)]
    pub network: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Information about a loaded plugin
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub metadata: PluginMetadata,
    pub capabilities: Vec<PluginCapability>,
    pub manifest_path: std::path::PathBuf,
    pub wasm_path: std::path::PathBuf,
}

/// Error types for plugin system
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Failed to load plugin: {0}")]
    LoadError(String),
    
    #[error("Failed to initialize plugin: {0}")]
    InitError(String),
    
    #[error("Plugin execution failed: {0}")]
    ExecutionError(String),
    
    #[error("Capability denied: {0:?}")]
    CapabilityDenied(PluginCapability),
    
    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),
    
    #[error("Plugin not found: {0}")]
    NotFound(String),
    
    #[error("WASM error: {0}")]
    WasmError(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("TOML parse error: {0}")]
    TomlError(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, PluginError>;