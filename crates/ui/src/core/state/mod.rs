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

/// Core application state.
///
/// Everything below is a mirror of what `arclain_app::ArclainApp` owns,
/// filled by `take_legacy_composition` and refreshed by
/// [`AppState::refresh_settings_from_facade`]. Reading a *setting* no
/// longer goes through here -- the settings snapshot lives on
/// [`AppSignals`] in the application's own DTO shapes. What is left is
/// composition plumbing that retires with `take_legacy_composition`
/// itself (`backend_selector`, `fallback_backend`, `db_paths`, `dbs`),
/// plus two fields no facade surface can serve yet:
///
/// - `pass_rules` carries the *decrypted passwords*
///   `arclain_core::utilities::auto_password_for` needs to unlock an
///   archive on open. `ArclainApp::password_rules` returns
///   `PasswordRuleSummary`, which deliberately reports only whether a
///   password is configured -- so auto-unlock cannot go through it, and
///   this stays until the facade owns auto-unlock itself.
/// - `user_config` still backs the settings pages' dirty-state checks
///   (`SettingsFeature::check_changes`), which compare a draft form
///   value against the stored one field by field.
pub struct AppState {
    /// User configuration loaded from database
    pub user_config: UserConfig,
    /// Password rules loaded from encrypted secrets DB, passwords
    /// included -- see this struct's own doc comment.
    pub pass_rules: Vec<PassRule>,
    pub backend_selector: BackendSelector,
    pub fallback_backend: SevenZipCli, // Keep for plugin compatibility
    /// Dead weight since the browser's duplicate listing pipeline was
    /// deleted: nothing writes this anymore (the relist used to stamp
    /// the just-listed archive's entry paths here for the auto-password
    /// matcher's entry-filename rung, which the facade's own ladders now
    /// cover with the session's real paths), and its one remaining
    /// reader is the unreachable pre-facade `convert_archive`. Dies with
    /// that function's process-page migration.
    pub last_entries: Vec<String>,

    pub encrypted_crc_policy: String,
    // DB-backed settings and secrets (optional; falls back to JSON if unavailable)
    pub db_paths: Option<DbPaths>,
    pub dbs: Option<ConfigDbs>,
    /// Reactive signals for async state updates
    pub signals: AppSignals,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}
