//! UI-side implementation of `arclain_plugins::ActiveTabBridge`.
//!
//! Resolves every call through `AppSignals.tabs.get().active()` so
//! the plugin manager / host functions always see the *currently
//! active* tab — no per-frame sync block, no captured-at-init
//! handles that go stale after `replace_active` / `col.open` /
//! `restore_tabs_on_launch`. See `arclain_plugins::active_tab` for
//! the design rationale.

use crate::core::signals::AppSignals;
use arclain_plugins::ActiveTabBridge;
use arclain_signals::Signal;
use std::path::PathBuf;

/// Bridge implementation that resolves through `AppSignals` on each
/// call. Lives for the lifetime of the plugin manager; the captured
/// `signals` clone is cheap (every field is `Arc`-internal).
pub struct AppSignalsBridge {
    signals: AppSignals,
}

impl AppSignalsBridge {
    pub fn new(signals: AppSignals) -> Self {
        Self { signals }
    }
}

impl ActiveTabBridge for AppSignalsBridge {
    fn archive_path(&self) -> Option<String> {
        self.signals
            .tabs
            .get()
            .active()
            .archive_path
            .get()
            .map(|p| p.to_string_lossy().into_owned())
    }

    fn current_password(&self) -> Option<String> {
        self.signals.tabs.get().active().current_password.get()
    }

    fn metadata_signal(&self) -> Signal<Option<serde_json::Value>> {
        self.signals.tabs.get().active().metadata.clone()
    }

    fn set_archive_path(&self, path: Option<String>) {
        self.signals
            .tabs
            .get()
            .active()
            .archive_path
            .set(path.map(PathBuf::from));
    }
}
