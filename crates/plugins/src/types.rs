//! Core types for the plugin system

use arclain_core::ArchiveKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A plugin identity that is safe to use as a portable filename component.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PluginId(String);

/// ASCII-case-folded identity used for registry and filesystem collisions.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PluginIdentityKey(String);

impl PluginId {
    /// Parse a plugin identity accepted by every supported filesystem.
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

    pub(crate) fn identity_key(&self) -> PluginIdentityKey {
        PluginIdentityKey(self.0.to_ascii_lowercase())
    }
}

impl PluginIdentityKey {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        PluginId::parse(value.to_owned()).map(|plugin_id| plugin_id.identity_key())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn join_under(&self, root: &Path) -> PathBuf {
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
    /// Create plugin-owned temporary files and delete content-cache entries.
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

/// Capabilities required for a plugin-originated background metadata fetch.
pub const REQUEST_FETCH_CAPABILITIES: [PluginCapability; 2] = [
    PluginCapability::Network,
    PluginCapability::ArchiveMetadataWrite,
];

/// Events that plugins can subscribe to.
///
/// Only `OnArchiveOpen` is wired through the dispatch worker today;
/// the other lifecycle variants (close, list, extract, etc.) were
/// dropped in the 2026-05-19 audit because the worker silently
/// ignored them and no plugin handler had ever observed them. Add a
/// new variant only when the dispatch path is ready to forward it.
///
/// `archive_session_id` pins the event to the *specific archive session*
/// it was fired for (the application facade's opaque
/// `ArchiveSessionId::into_raw()` value -- this crate cannot name
/// `arclain_app::ids::ArchiveSessionId` itself without an illegal reverse
/// dependency, since `arclain_app` depends on this crate, not the other
/// way around), so the worker can route the plugin handler's host-function
/// reads (`current_archive_info`, `list_archive_files`) and metadata
/// writes (`emit_metadata`) to that session even if events queue up and
/// the user switches tabs in the meantime.
///
/// This replaces a previously-carried `arclain_signals::Signal<Option<
/// serde_json::Value>>` field: the application layer that now fires this
/// event (rather than `crates/ui`) has no UI-signal type to hand over, and
/// should not need one -- a plain, application-owned session id is the
/// right payload for an application-layer emitter to construct.
/// `ActiveTabBridge::set_session_metadata` is the write path that resolves
/// this id back to wherever its host stores that session's metadata.
///
/// The pre-existing serde derives on this enum were dead (no production
/// caller) and have been dropped -- there was no reader to satisfy anyway.
#[derive(Clone)]
pub enum PluginEvent {
    /// Archive was opened
    OnArchiveOpen {
        path: String,
        kind: ArchiveKind,
        password: Option<String>,
        /// In-archive entry paths for the originating session's archive,
        /// captured at fire time. Lets `list_archive_files` in the
        /// plugin handler return the originating session's entries
        /// instead of whatever's active in the bridge when the worker
        /// gets around to processing this event.
        ///
        /// Paths rather than listing rows because paths are all a guest
        /// can observe -- `list_archive_files` yields them and
        /// `archive_file_count` yields this list's length -- and because
        /// it makes this payload the same shape as the non-event path's
        /// `ActiveTabBridge::archive_entries`.
        entries: std::sync::Arc<Vec<String>>,
        /// The opaque `ArchiveSessionId` (raw `u64`) this event was fired
        /// for. See this enum's own doc comment.
        archive_session_id: u64,
    },
}

impl std::fmt::Debug for PluginEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginEvent::OnArchiveOpen {
                path,
                kind,
                password,
                entries,
                archive_session_id,
            } => f
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
    /// Sidebar panel section (renamed from InfoPanel — the legacy
    /// `Sidebar` variant + its `serde(alias = "Sidebar")` migration
    /// crutch were dropped in the 2026-05-20 audit Tier 2 cleanup;
    /// nothing was constructing `Sidebar` anymore. Plugins still
    /// match on the WIT string `"Sidebar"` defensively, but the host
    /// never emits it.)
    Panel,
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
    /// Show a toast notification
    ShowToast { message: String, level: ToastLevel },
    /// Request UI refresh for an extension point
    RefreshPanel { extension_point: String },
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

    #[error("Plugin unavailable: {0}")]
    Unavailable(String),

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

#[cfg(test)]
mod resource_limit_error_tests {
    use super::*;

    #[test]
    fn unavailable_error_exposes_only_the_redacted_host_reason() {
        let error = PluginError::Unavailable("fuel quota exceeded".to_string());

        assert_eq!(error.to_string(), "Plugin unavailable: fuel quota exceeded");
    }
}
/// Maximum structured metadata body that a plugin may publish or receive.
pub const MAX_PLUGIN_METADATA_BYTES: usize = 4 * 1024 * 1024;
/// Maximum opaque Data API body lifted across the component boundary.
pub const MAX_PLUGIN_GUEST_DATA_BYTES: usize = 4 * 1024 * 1024;

/// Opaque handle naming one archive an application-layer session has
/// made available to plugin host functions. Deliberately *not*
/// `arclain_app::ids::ArchiveSessionId` reused directly: `arclain_app`
/// depends on `arclain_plugins`, not the other way around, so a type
/// this crate's own public API names must be minted here -- an
/// `arclain_app`-side adapter is expected to construct one per
/// `ArchiveSessionId` it owns (typically by round-tripping the same raw
/// `u64`, the same way `PluginEvent::OnArchiveOpen::archive_session_id`
/// already does) rather than this crate depending on that id type
/// itself.
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

/// What one [`PluginArchiveContextId`] identifies: an archive's on-disk
/// path and kind, for a host function that needs to describe (rather
/// than read from) the archive a context id names.
pub struct PluginArchiveContext {
    pub context_id: PluginArchiveContextId,
    pub path: std::path::PathBuf,
    pub kind: ArchiveKind,
}

/// Application-owned, session-scoped archive access for plugin host
/// functions: list an archive's entries, read one entry's bytes, and
/// write back structured metadata -- everything a host function needs
/// to serve an archive-context WIT import without this crate holding
/// (or this crate's event payloads embedding) the archive's password or
/// its full entry collection directly. An `arclain_app`-side adapter
/// implements this over its own `ArchiveSessionStore`, resolving a
/// [`PluginArchiveContextId`] back to the concrete open session the same
/// way it already resolves an `ArchiveSessionId`.
///
/// Defined here (not in `arclain_app`) specifically to avoid an
/// `arclain_app`/`arclain_plugins` dependency cycle: `arclain_app`
/// depends on `arclain_plugins`, so a trait this crate's own host
/// functions are meant to call through must be declared on this side of
/// that edge, with `arclain_app` providing the implementation.
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

pub fn metadata_value_within_limit(value: &serde_json::Value) -> bool {
    struct LimitWriter {
        written: usize,
    }

    impl std::io::Write for LimitWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.written.saturating_add(buffer.len()) > MAX_PLUGIN_METADATA_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "metadata publication limit exceeded",
                ));
            }
            self.written += buffer.len();
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(&mut LimitWriter { written: 0 }, value).is_ok()
}
