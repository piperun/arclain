//! Types for plugin management

use crate::runtime::PluginInstance;
use crate::types::PluginMetadata;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Validated package metadata shown before the caller approves installation.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginInstallPreview {
    pub manifest: crate::types::PluginManifest,
    pub fingerprint: wirt::PackageFingerprint,
}

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
    /// Cloned from the instance's `HostFunctions::settings_dirty` so
    /// `get_all_settings` can probe it without taking
    /// `instance.lock()` (audit P14).
    pub(crate) settings_dirty: Arc<AtomicBool>,
}

/// Persisted settings indexed internally by a case-folded plugin identity.
/// The original spelling remains available for persistence/display output.
pub(crate) struct InitialPluginSettings {
    pub(crate) original_id: String,
    pub(crate) values: HashMap<String, String>,
}

/// One plugin discovered on disk that failed to load, recorded so
/// [`super::PluginManager::failed_plugins`] can report it -- the
/// application-facade `PluginSummary.load_error` field this backs did
/// not previously have any host-side source: `PluginManager::init`
/// logged a load failure and otherwise discarded it, so an install that
/// silently failed (a stale manifest, a corrupted `.wasm`) was invisible
/// to anything but the log file.
#[derive(Clone, Debug)]
pub struct FailedPlugin {
    /// The plugin id as it appeared in the manifest that failed to load
    /// -- not necessarily a validated [`crate::types::PluginId`], since
    /// an invalid id is itself one of the ways loading can fail.
    pub original_id: String,
    /// The host-generated failure reason. Never a guest-controlled
    /// string: `load_plugin`'s own error paths (manifest parsing,
    /// capability validation, WASM instantiation) only ever produce
    /// `PluginError`'s own `Display` text.
    pub error: String,
}
