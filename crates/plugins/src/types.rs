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
    OnArchiveOpen { path: String, kind: ArchiveKind },
    /// Archive was closed
    OnArchiveClose { path: String },
    /// Archive contents were listed
    OnArchiveList {
        path: String,
        entries: Vec<ArchiveEntry>,
    },
    /// File was extracted from archive
    OnFileExtract { archive: String, file_path: String },
    /// File was opened from archive
    OnFileOpen { archive: String, file_path: String },
    /// File was added to archive
    OnFileAdd { archive: String, file_path: String },
    /// File was deleted from archive
    OnFileDelete { archive: String, file_path: String },
    /// Metadata display requested
    OnMetadataDisplay { archive: String },
}

/// Response from a plugin after handling an event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginResponse {
    /// No response needed
    None,
    /// Plugin provided metadata
    Metadata { data: serde_json::Value },
    /// Plugin encountered an error
    Error { message: String },
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

/// Extension point where a plugin provides UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginExtensionPoint {
    /// Main page when plugin is selected in Plugins page
    MainPage,
    /// Widget to inject into archive properties sidebar
    Sidebar,
    /// Future: context menu items
    ContextMenu,
}

/// UI element that a plugin can define
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginUiElement {
    /// Vertical layout container
    Column {
        #[serde(default)]
        children: Vec<PluginUiElement>,
    },
    /// Horizontal layout container
    Row {
        #[serde(default)]
        children: Vec<PluginUiElement>,
    },
    /// Text label
    Label {
        text: String,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        size: Option<f32>,
    },
    /// Button
    Button { id: String, label: String },
    /// Text input
    TextInput {
        id: String,
        label: String,
        value: String,
    },
    /// Checkbox
    Checkbox {
        id: String,
        label: String,
        checked: bool,
    },
    /// Radio button group
    RadioGroup {
        id: String,
        label: String,
        options: Vec<String>,
        selected: String,
    },
    /// Slider
    Slider {
        id: String,
        label: String,
        value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
    },
    /// Dropdown menu
    Dropdown {
        id: String,
        label: String,
        options: Vec<String>,
        selected: String,
    },
    /// Separator line
    Separator,
    /// Spacing
    Space {
        #[serde(default = "default_space_size")]
        size: f32,
    },
}

fn default_space_size() -> f32 {
    8.0
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

// Conversion from WIT types to Core types
use crate::bindings::arclain::plugin::rules as wit_rules;

impl From<wit_rules::PluginRuleDefinition> for arclain_core::OrganizationRule {
    fn from(def: wit_rules::PluginRuleDefinition) -> Self {
        arclain_core::OrganizationRule {
            name: def.name,
            priority: 100, // Plugins get high priority by default? Or config?
            is_enabled: true,
            trigger: def.trigger.into(),
            actions: def.actions.into(),
        }
    }
}

impl From<wit_rules::PluginRuleTrigger> for arclain_core::RuleTrigger {
    fn from(t: wit_rules::PluginRuleTrigger) -> Self {
        arclain_core::RuleTrigger {
            filename_pattern: t.filename_pattern,
            has_file: t.has_file,
            metadata_source: t.metadata_source,
        }
    }
}

impl From<wit_rules::PluginRuleActions> for arclain_core::RuleActions {
    fn from(a: wit_rules::PluginRuleActions) -> Self {
        arclain_core::RuleActions {
            root_folder: a.root_folder,
            move_files: a.move_files.into_iter().map(|m| m.into()).collect(),
            use_standard_layout: a.use_standard_layout,
        }
    }
}

impl From<wit_rules::MoveFileRule> for arclain_core::MoveAction {
    fn from(m: wit_rules::MoveFileRule) -> Self {
        arclain_core::MoveAction {
            pattern: m.pattern,
            target: m.target,
        }
    }
}
