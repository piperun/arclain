//! Plugin UI type definitions

use serde::{Deserialize, Serialize};

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
}

impl PluginsListState {
    /// Update plugin list from plugin manager
    pub fn update_from_manager(
        &mut self,
        manager: &arclain_plugins::PluginManager,
        user_config: &arclain_core::UserConfig,
    ) {
        self.plugins.clear();

        let visibility_json = user_config.plugin_visibility.as_deref().unwrap_or("{}");
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
