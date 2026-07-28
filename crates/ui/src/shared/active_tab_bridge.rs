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

    fn archive_entries(&self) -> Vec<String> {
        // tab.entries is `Signal<Arc<Vec<ArchiveEntry>>>`; .get()
        // returns the Arc, .iter() walks it, and we materialize the
        // path strings the plugin will consume. Cheap relative to
        // the alternative (re-listing via backend.list, which for
        // 7z spawns a subprocess and takes seconds).
        self.signals
            .tabs
            .get()
            .active()
            .entries
            .get()
            .iter()
            .map(|e| e.path.clone())
            .collect()
    }

    fn archive_entry_count(&self) -> usize {
        self.signals.tabs.get().active().entries.get().len()
    }

    fn archive_entries_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.signals
            .tabs
            .get()
            .active()
            .entries
            .get()
            .iter()
            .skip(offset)
            .take(limit)
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn metadata_signal(&self) -> Signal<Option<serde_json::Value>> {
        self.signals.tabs.get().active().metadata.clone()
    }

    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>) {
        // Resolved by session id, not "whichever tab is active now" -- the
        // whole point of moving `OnArchiveOpen` to carry an
        // `archive_session_id` instead of a captured UI signal (see
        // `arclain_plugins::PluginEvent::OnArchiveOpen`'s doc comment) is
        // that a queued event's metadata must land on the tab that
        // requested it, even if the user has since switched tabs. A tab
        // whose session was since closed (or that never matched -- for
        // example a stale event outliving its tab) is a silent no-op,
        // matching the pre-existing behavior for an event whose
        // originating tab had already closed by the time the worker
        // processed it.
        let target_id = arclain_app::ids::ArchiveSessionId::from_raw(archive_session_id);
        let tabs = self.signals.tabs.get();
        if let Some(tab) = tabs
            .tabs()
            .iter()
            .find(|tab| tab.archive_session_id.get() == Some(target_id))
        {
            tab.metadata.set(metadata);
        }
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
