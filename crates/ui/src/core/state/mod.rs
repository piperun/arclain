//! Application state management
//!
//! This module contains the core AppState struct and related types.
//! The implementation is split across multiple files for maintainability:
//! - `init` - State initialization (`AppState::new()`)
//! - `vault_ops` - Vault and preferences management
//! - `config_ops` - Configuration sync and reload

mod config_ops;
mod init;
mod vault_ops;

use super::signals::AppSignals;

/// Core application state.
///
/// Backend services, databases, configuration, archive backends, and
/// decrypted password rules are owned by `arclain_app::ArclainApp`.
/// The egui frontend retains only its reactive presentation signals.
pub struct AppState {
    /// Reactive signals for async state updates
    pub signals: AppSignals,
}

/// UI display preferences (persisted to config DB)
#[derive(Clone, Default)]
pub struct UiPreferences {
    /// Show text labels on header/toolbar buttons
    pub show_button_labels: bool,
}
