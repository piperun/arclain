//! Types for plugin management

use crate::runtime::PluginInstance;
use crate::types::PluginMetadata;
use parking_lot::Mutex;
use std::sync::Arc;

/// Information about a plugin for UI display
#[derive(Clone, Debug)]
pub struct PluginListItem {
    pub id: String,
    pub manifest: crate::types::PluginManifest,
    pub enabled: bool,
    pub instance: Option<()>, // Just a marker for whether it's loaded
}

/// Cheap counts-only snapshot returned by [`super::PluginManager::status_summary`].
///
/// Avoids cloning per-plugin manifests when the caller only needs
/// totals (e.g. status bar rendering every frame).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginStatusSummary {
    pub total: usize,
    pub enabled: usize,
}

/// A managed plugin with its instance and metadata
pub(crate) struct ManagedPlugin {
    pub(crate) metadata: PluginMetadata,
    pub(crate) instance: Arc<Mutex<PluginInstance>>,
    pub(crate) manifest: crate::types::PluginManifest,
    pub(crate) enabled: bool,
}
