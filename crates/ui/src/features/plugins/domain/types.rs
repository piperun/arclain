//! Plugin UI type definitions

use arclain_plugins::types::PluginLayout;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_PLUGIN_UI_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

impl RequestId {
    pub(crate) fn next() -> Self {
        Self(NEXT_PLUGIN_UI_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// UI representation of a plugin
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Plugin identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Version string
    pub version: String,
    /// Author name
    pub author: Option<String>,
    /// Description
    pub description: Option<String>,
    /// Required capabilities
    pub capabilities: Vec<String>,
    /// Whether plugin is currently enabled
    pub enabled: bool,
    /// Whether plugin is loaded successfully
    pub loaded: bool,
    /// Current plugin status
    pub status: PluginStatus,
    /// Error message if any
    pub error: Option<String>,
    /// Visibility settings (e.g. "toolbar": true)
    #[serde(default)]
    pub visibility: std::collections::HashMap<String, bool>,
}

/// Plugin status indicator
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginStatus {
    /// Plugin not yet loaded
    NotLoaded,
    /// Plugin is being loaded
    Loading,
    /// Plugin loaded and ready
    Ready,
    /// Plugin currently processing
    Running,
    /// Plugin encountered an error
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SnapshotStatus {
    #[default]
    Idle,
    Pending,
    Ready,
    Failed(String),
}

impl PluginStatus {
    /// Get icon for status
    pub fn icon(&self) -> &'static str {
        match self {
            PluginStatus::NotLoaded => "○",
            PluginStatus::Loading => "⟳",
            PluginStatus::Ready => "●",
            PluginStatus::Running => "▶",
            PluginStatus::Error => "⚠",
        }
    }

    /// Get color for status
    pub fn color(&self) -> egui::Color32 {
        match self {
            PluginStatus::NotLoaded => egui::Color32::GRAY,
            PluginStatus::Loading => egui::Color32::from_rgb(100, 150, 255),
            PluginStatus::Ready => egui::Color32::from_rgb(100, 200, 100),
            PluginStatus::Running => egui::Color32::from_rgb(255, 200, 50),
            PluginStatus::Error => egui::Color32::from_rgb(255, 100, 100),
        }
    }
}

/// State for plugins list view
#[derive(Clone, Debug, Default)]
pub struct PluginsListState {
    /// List of all plugins
    pub plugins: Vec<PluginInfo>,
    /// Currently selected plugin ID
    pub selected_plugin: Option<String>,
    /// Whether to show disabled plugins
    pub show_disabled: bool,
    /// Whether to show permission tags in the list
    pub show_permissions: bool,
    /// Filter text for searching
    pub filter_text: String,
    /// Cached `MainPage` layout for the currently-selected plugin.
    /// Keyed as `(plugin_id, layout)`. Populated lazily on first
    /// render and invalidated when the selected plugin changes or
    /// the plugin sends a `RefreshPanel` action targeting `MainPage`.
    /// Without this, every render frame issued a WASM
    /// `get_ui_layout(MainPage)` call into the plugin (audit P4).
    pub cached_main_layout: Option<(String, Arc<PluginLayout>)>,
    pub snapshot_status: SnapshotStatus,
    pub snapshot_request_id: Option<RequestId>,
    /// The last value of `AppSignals::plugin_list_epoch` this state
    /// synced its snapshot against. `plugins_page::render` compares this
    /// to the current shared epoch on every render and invalidates the
    /// snapshot on a mismatch -- see that comparison's own doc comment
    /// for why a shared epoch, not a direct cross-state invalidation,
    /// is what keeps `PluginsFeature`'s two independent `PluginsListState`
    /// instances (the standalone Plugins page and the Plugins settings
    /// page) from showing a stale `enabled` flag for each other's most
    /// recent toggle.
    pub plugin_list_epoch_seen: u64,
}

impl PluginsListState {
    pub fn invalidate_snapshot(&mut self) {
        self.snapshot_status = SnapshotStatus::Idle;
        self.snapshot_request_id = None;
    }

    pub fn apply_snapshot(&mut self, request_id: RequestId, plugins: Vec<PluginInfo>) -> bool {
        if self.snapshot_request_id != Some(request_id) {
            return false;
        }
        self.plugins = plugins;
        self.snapshot_status = SnapshotStatus::Ready;
        self.snapshot_request_id = None;
        true
    }

    pub fn apply_snapshot_failure(&mut self, request_id: RequestId, error: String) -> bool {
        if self.snapshot_request_id != Some(request_id) {
            return false;
        }
        self.snapshot_status = SnapshotStatus::Failed(error);
        self.snapshot_request_id = None;
        true
    }
}

impl PluginsListState {
    /// Update plugin list from plugin manager
    /// `plugin_visibility` is the stored per-plugin visibility blob
    /// (a `{plugin_id: {slot: bool}}` JSON object), or `None` when
    /// nothing has been stored. Unparseable content is treated the same
    /// as absent -- a corrupt blob hides no plugin.
    pub fn update_from_manager(
        &mut self,
        manager: &arclain_plugins::PluginManager,
        plugin_visibility: Option<&str>,
    ) {
        self.plugins.clear();

        let visibility_json = plugin_visibility.unwrap_or("{}");
        let visibility_map: std::collections::HashMap<
            String,
            std::collections::HashMap<String, bool>,
        > = serde_json::from_str(visibility_json).unwrap_or_default();

        let mut unsorted_plugins = Vec::new();

        for item in manager.list_plugins() {
            let caps = item.manifest.capabilities.to_capabilities();
            let cap_strings: Vec<String> = caps.iter().map(|c| format!("{:?}", c)).collect();

            let plugin_vis = visibility_map.get(&item.id).cloned().unwrap_or_default();

            let info = PluginInfo {
                id: item.id.clone(),
                name: item.manifest.plugin.name.clone(),
                version: item.manifest.plugin.version.clone(),
                author: Some(item.manifest.plugin.author.clone()),
                description: Some(item.manifest.plugin.description.clone()),
                capabilities: cap_strings,
                enabled: item.enabled,
                loaded: item.instance.is_some(),
                status: if item.instance.is_some() {
                    PluginStatus::Ready
                } else {
                    PluginStatus::NotLoaded
                },
                error: None,
                visibility: plugin_vis,
            };
            unsorted_plugins.push(info);
        }

        // Sort plugins alphabetically (cached key avoids per-comparison allocation)
        unsorted_plugins.sort_by_cached_key(|p| p.name.to_lowercase());

        self.plugins = unsorted_plugins;
    }
}
