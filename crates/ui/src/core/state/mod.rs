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
    // Plugin system - event sender stays for dispatch, manager moved to Services
    /// Event sender for non-blocking plugin dispatch (no mutex lock needed)
    pub plugin_event_sender: Option<std::sync::mpsc::Sender<arclain_plugins::PluginEvent>>,
    /// Pending plugin event to dispatch after UI is ready.
    /// This is set when an archive opens and cleared after the UI renders
    /// and dispatches the event.
    pub pending_plugin_event: Option<arclain_plugins::PluginEvent>,
    /// Reactive signals for async state updates
    pub signals: AppSignals,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}
