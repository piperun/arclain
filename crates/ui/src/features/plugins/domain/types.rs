//! Plugin UI type definitions

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
    #[serde(default)]
    pub quarantine_state: arclain_app::plugins::PluginQuarantineState,
    #[serde(default)]
    pub last_reason: Option<String>,
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

/// One user-selected package from inspection through explicit approval.
#[derive(Clone, Debug)]
pub struct PendingPluginInstall {
    pub package_path: PathBuf,
    pub preview: Option<arclain_app::plugins::PluginInstallPreviewDto>,
    pub request_id: Option<RequestId>,
    pub loading: bool,
    pub installing: bool,
    pub error_kind: Option<arclain_app::error::ApplicationErrorKind>,
    pub error: Option<String>,
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
    /// The permission review currently owned by this page, if any.
    pub pending_install: Option<PendingPluginInstall>,
}

impl PluginsListState {
    pub fn begin_package_inspection(&mut self, package_path: PathBuf, request_id: RequestId) {
        self.pending_install = Some(PendingPluginInstall {
            package_path,
            preview: None,
            request_id: Some(request_id),
            loading: true,
            installing: false,
            error_kind: None,
            error: None,
        });
    }

    pub fn apply_package_preview(
        &mut self,
        request_id: RequestId,
        preview: arclain_app::plugins::PluginInstallPreviewDto,
    ) -> bool {
        let Some(pending) = self.pending_install.as_mut() else {
            return false;
        };
        if pending.request_id != Some(request_id) || pending.installing {
            return false;
        }
        pending.preview = Some(preview);
        pending.request_id = None;
        pending.loading = false;
        pending.error_kind = None;
        pending.error = None;
        true
    }

    pub fn begin_package_install(&mut self, request_id: RequestId) -> bool {
        let Some(pending) = self.pending_install.as_mut() else {
            return false;
        };
        if pending.preview.is_none() || pending.loading {
            return false;
        }
        pending.request_id = Some(request_id);
        pending.loading = true;
        pending.installing = true;
        pending.error_kind = None;
        pending.error = None;
        true
    }

    pub fn apply_package_install_failure(
        &mut self,
        request_id: RequestId,
        error_kind: Option<arclain_app::error::ApplicationErrorKind>,
        error: String,
    ) -> bool {
        let Some(pending) = self.pending_install.as_mut() else {
            return false;
        };
        if pending.request_id != Some(request_id) {
            return false;
        }
        pending.request_id = None;
        pending.loading = false;
        pending.installing = false;
        pending.error_kind = error_kind;
        pending.error = Some(error);
        true
    }

    pub fn complete_package_install(&mut self, request_id: RequestId) -> bool {
        if !matches!(
            self.pending_install.as_ref(),
            Some(pending) if pending.request_id == Some(request_id) && pending.installing
        ) {
            return false;
        }
        self.pending_install = None;
        true
    }

    pub fn cancel_package_install(&mut self) -> bool {
        if matches!(
            self.pending_install.as_ref(),
            Some(pending) if pending.installing
        ) {
            return false;
        }
        self.pending_install.take().is_some()
    }

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
