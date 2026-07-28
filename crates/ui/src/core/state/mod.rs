//! Application state management
//!
//! This module contains the core AppState struct and related types.
//! The implementation is split across multiple files for maintainability:
//! - `init` - State initialization (`AppState::new()`)
//! - `archive_ops` - Archive listing and file operations
//! - `vault_ops` - Vault and preferences management
//! - `password_ops` - Password rules management
//! - `config_ops` - Configuration sync and reload

mod archive_ops;
mod config_ops;
mod init;
mod password_ops;
mod vault_ops;

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::backends::BackendSelector;
use arclain_core::utilities::PassRule;
use arclain_core::UserConfig;
use arclain_core::{ConfigDbs, DbPaths};

use super::signals::AppSignals;
use super::tabs::TabId;

/// Plugin event captured together with the tab that originated it.
///
/// The event payload already carries tab-local archive data and metadata
/// handles. Keeping the `TabId` alongside it lets the deferred UI dispatcher
/// release the matching tab's `ui_ready` gate after queueing the event, even
/// when another tab became active in the meantime.
pub struct PendingPluginEvent {
    pub origin_tab_id: TabId,
    pub event: arclain_plugins::PluginEvent,
}

impl PendingPluginEvent {
    pub fn new(origin_tab_id: TabId, event: arclain_plugins::PluginEvent) -> Self {
        Self {
            origin_tab_id,
            event,
        }
    }
}

/// Core application state
pub struct AppState {
    /// User configuration loaded from database
    pub user_config: UserConfig,
    /// Password rules loaded from encrypted secrets DB
    pub pass_rules: Vec<PassRule>,
    pub backend_selector: BackendSelector,
    pub fallback_backend: SevenZipCli, // Keep for plugin compatibility
    pub last_entries: Vec<String>,

    pub encrypted_crc_policy: String,
    // DB-backed settings and secrets (optional; falls back to JSON if unavailable)
    pub db_paths: Option<DbPaths>,
    pub dbs: Option<ConfigDbs>,
    // Plugin system - scheduler stays for dispatch, manager moved to Services
    /// Cloneable bounded scheduler used non-blockingly on the UI path.
    pub plugin_event_scheduler: Option<arclain_plugins::PluginEventScheduler>,
    /// Bounded/coalesced archive events awaiting non-blocking worker admission.
    /// At most the newest event for each tab is retained.
    pub pending_plugin_events: Vec<PendingPluginEvent>,
    /// Reactive signals for async state updates
    pub signals: AppSignals,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}
