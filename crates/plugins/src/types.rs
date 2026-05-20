//! Core types for the plugin system

use arclain_core::ArchiveKind;
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

/// Events that plugins can subscribe to.
///
/// Only `OnArchiveOpen` is wired through the dispatch worker today;
/// the other lifecycle variants (close, list, extract, etc.) were
/// dropped in the 2026-05-19 audit because the worker silently
/// ignored them and no plugin handler had ever observed them. Add a
/// new variant only when the dispatch path is ready to forward it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginEvent {
    /// Archive was opened
    OnArchiveOpen {
        path: String,
        kind: ArchiveKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        password: Option<String>,
    },
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
    /// List of domains the plugin is allowed to access (auto-approved on load)
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginExtensionPoint {
    /// Main plugin settings page
    MainPage,
    /// Toolbar button slot
    PluginButton,
    /// Context menu items
    ContextMenu,
    /// Sidebar panel section (renamed from InfoPanel)
    Panel,
    /// Deprecated - use Panel
    #[serde(alias = "Sidebar")]
    Sidebar,
    /// Plugin settings page
    Settings,
    /// Modal dialog (parameterized by ID)
    Dialog(String),
    /// Full page view (parameterized by ID)
    Page(String),
}

/// Button action for declarative navigation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ButtonAction {
    None,
    ShowDialog { id: String },
    CloseDialog,
    OpenPage { id: String },
    ClosePage,
    Custom(String),
}

impl Default for ButtonAction {
    fn default() -> Self {
        ButtonAction::None
    }
}

/// UI element that a plugin can define
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginUiElement {
    /// Vertical layout container
    Column {
        #[serde(default)]
        children: Vec<PluginUiElement>,
        #[serde(default)]
        spacing: Option<f32>,
    },
    /// Horizontal layout container
    Row {
        #[serde(default)]
        children: Vec<PluginUiElement>,
        #[serde(default)]
        spacing: Option<f32>,
    },
    /// Grid layout container
    Grid {
        columns: u32,
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
    /// Section header for semantic title hierarchy (h1-h4 style)
    SectionHeader {
        title: String,
        /// 1=largest (h1), 4=smallest (h4)
        level: u32,
        #[serde(default)]
        description: Option<String>,
    },
    /// Button with optional navigation action
    Button {
        id: String,
        label: String,
        #[serde(default)]
        action: Option<ButtonAction>,
    },
    /// Text input
    TextInput {
        id: String,
        label: String,
        value: String,
        /// If set, renders as simple input with placeholder (no label title)
        #[serde(default)]
        placeholder: Option<String>,
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
    /// Image display
    Image {
        #[serde(default)]
        cache_key: Option<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        max_height: Option<f32>,
    },
    /// Separator line
    Separator,
    /// Spacing
    Space {
        #[serde(default = "default_space_size")]
        size: f32,
    },
    /// Tabs for view switching
    Tabs {
        id: String,
        tabs: Vec<String>,
        selected: String,
    },
    /// List item (card-like display)
    ListItem {
        id: String,
        title: String,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        badge: Option<String>,
        #[serde(default)]
        image_key: Option<String>,
        #[serde(default)]
        image_url: Option<String>,
        #[serde(default)]
        selected: bool,
        #[serde(default)]
        warning_icon: Option<WarningIcon>,
    },
    /// Scrollable list container
    ListContainer {
        id: String,
        items: Vec<PluginUiElement>, // Should contain ListItem elements
        #[serde(default)]
        max_height: Option<f32>,
        #[serde(default)]
        empty_message: Option<String>,
    },
    /// Loading indicator
    Loading {
        #[serde(default)]
        message: Option<String>,
    },
    /// Marker: open a visually-grouped settings section with this title.
    /// Pair with a later `GroupEnd` marker; everything between the two
    /// markers is wrapped in the host's standard Form/SettingsGroup container
    /// so plugin-supplied panels match the rest of the app.
    GroupBegin {
        title: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// Marker: close the most recently opened `GroupBegin`.
    GroupEnd,
    /// Warning / Alert banner
    Warning { icon: WarningIcon, message: String },
    /// Tag chips displayed as styled pills
    TagChips {
        tags: Vec<String>,
        #[serde(default)]
        max_display: Option<u32>,
    },
    /// Toolbar with buttons
    Toolbar { buttons: Vec<ToolbarButton> },
    /// Carousel gallery with thumbnail strip
    Carousel {
        id: String,
        /// List of images: (cache_key, optional_url)
        images: Vec<(String, Option<String>)>,
        /// Currently selected image index
        current_index: usize,
        /// Max height for main image (default 300)
        #[serde(default)]
        max_height: Option<f32>,
        /// Thumbnail height (default 60)
        #[serde(default)]
        thumbnail_height: Option<f32>,
        /// Enable click-to-open lightbox (default true)
        #[serde(default = "default_true")]
        enable_lightbox: bool,
    },
    /// Key-value list for displaying metadata in a two-column grid (label: value)
    KeyValueList {
        items: Vec<KeyValuePair>,
        /// Number of key-value pairs per row (default: 1)
        #[serde(default)]
        columns: Option<u32>,
    },
    /// Metadata grid for displaying key-value metadata in card format (label above value)
    MetadataGrid {
        items: Vec<KeyValuePair>,
        /// Number of columns (default: auto-fit based on width)
        #[serde(default)]
        columns: Option<u32>,
    },
}

/// Key-value pair for metadata display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValuePair {
    pub key: String,
    pub value: String,
}

/// Toolbar button configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolbarButton {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub primary: bool,
    /// Add flexible space before this button (pushes it to the right)
    #[serde(default)]
    pub spacer_before: bool,
}

/// Warning icon type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WarningIcon {
    Warning,
    GlobeX,
}

/// Toast notification level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// Action that a plugin can request from the host
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginAction {
    /// No action
    None,
    /// Request host to cache content from URL
    CacheContent { key: String, url: String },
    /// Show a toast notification
    ShowToast { message: String, level: ToastLevel },
    /// Show a message dialog
    ShowMessage { title: String, message: String },
    /// Request UI refresh for an extension point
    RefreshPanel { extension_point: String },
    /// Update a specific element's value
    UpdateElement { id: String, value: String },
    /// Navigate to a plugin page
    OpenPage { page: String },
    /// Close the current dialog
    CloseDialog,
    /// Copy text to system clipboard
    CopyToClipboard { text: String },
    /// Open the lightbox with images
    OpenLightbox {
        /// List of images: (cache_key, optional_url)
        images: Vec<(String, Option<String>)>,
        /// Starting image index
        start_index: usize,
        /// Optional title for the lightbox
        title: Option<String>,
    },
    /// Set the display name for the current plugin page (shown in breadcrumbs/title)
    SetPageDisplayName { name: String },
    /// Request async background fetch — host handles on tokio runtime.
    /// Format: "source:id" e.g. "dlsite:RJ123456"
    RequestFetch { key: String },
}

fn default_space_size() -> f32 {
    8.0
}

fn default_true() -> bool {
    true
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

// === Top Tab Registration Types ===

/// Badge configuration for tabs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BadgeConfig {
    pub count: Option<u32>,
    pub dot: bool,
    pub color: String,
}

/// Top-level tab configuration from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTabConfig {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub badge: Option<BadgeConfig>,
    pub priority: u32,
}

// === Layout Types ===

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginLayout {
    Single {
        elements: Vec<PluginUiElement>,
    },
    Split {
        sidebar: Vec<PluginUiElement>,
        content: Vec<PluginUiElement>,
        #[serde(default)]
        sidebar_width: Option<f32>,
    },
}

impl Default for PluginLayout {
    fn default() -> Self {
        PluginLayout::Single { elements: vec![] }
    }
}

impl PluginLayout {
    pub fn is_empty(&self) -> bool {
        match self {
            PluginLayout::Single { elements } => elements.is_empty(),
            PluginLayout::Split {
                sidebar, content, ..
            } => sidebar.is_empty() && content.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            PluginLayout::Single { elements } => elements.len(),
            PluginLayout::Split {
                sidebar, content, ..
            } => sidebar.len() + content.len(),
        }
    }

    /// Returns a flat list of elements if it's a Single layout, or concatenates sidebar+content if Split.
    /// Useful for legacy views that expect a single list.
    pub fn flatten(self) -> Vec<PluginUiElement> {
        match self {
            PluginLayout::Single { elements } => elements,
            PluginLayout::Split {
                mut sidebar,
                mut content,
                ..
            } => {
                sidebar.append(&mut content);
                sidebar
            }
        }
    }
}
