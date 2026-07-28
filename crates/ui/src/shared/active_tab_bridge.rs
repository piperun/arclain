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
use std::path::PathBuf;

/// Upper bound on how many sessions' worth of metadata
/// `set_session_metadata` will hold in `AppSignals::
/// pending_session_metadata` while waiting for their tab to be
/// stamped. `set_session_metadata` is a WASM plugin host function --
/// any plugin can call it with an arbitrary `archive_session_id` that
/// will never actually be stamped (a made-up id, a bug, or a hostile
/// plugin calling it in a loop), and every such call would otherwise
/// grow the buffer forever, since nothing ever drains an entry whose
/// session never completes. Comfortably above any realistic number of
/// archives a user could have genuinely opening at once across every
/// tab; reaching it is itself a signal something is wrong, not normal
/// use -- so the response is to drop the *new* entry and log a
/// warning, not silently evict a legitimate older one that might still
/// be about to be claimed.
const MAX_PENDING_SESSION_METADATA: usize = 64;

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

    fn active_archive_session_id(&self) -> Option<u64> {
        self.signals
            .tabs
            .get()
            .active()
            .archive_session_id
            .get()
            .map(arclain_app::ids::ArchiveSessionId::into_raw)
    }

    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>) {
        // Resolved by session id, not "whichever tab is active now" -- the
        // whole point of moving `OnArchiveOpen` to carry an
        // `archive_session_id` instead of a captured UI signal (see
        // `arclain_plugins::PluginEvent::OnArchiveOpen`'s doc comment) is
        // that a queued event's metadata must land on the tab that
        // requested it, even if the user has since switched tabs.
        //
        // No match can mean two very different things:
        //
        //   1. The tab's session was since closed (or the event is
        //      stale, outliving a tab that already closed by the time
        //      this ran) -- there is nothing left to update; dropping
        //      the write is correct.
        //   2. The operation-bridge worker hasn't yet stamped the
        //      originating tab with this session id -- a plugin's
        //      `OnArchiveOpen` handler can call back into this host
        //      function before `crate::core::operation_bridge::
        //      handle_open_archive_completed` (a fully independent
        //      consumer of the same operation-completion event) gets
        //      around to it. Dropping the write here would silently
        //      lose the very first metadata a plugin ever computes for
        //      a freshly-opened archive.
        //
        // This function cannot tell which case it's in, so it always
        // buffers into `pending_session_metadata` alongside applying
        // directly wherever a match already exists. `handle_open_archive_
        // completed` drains (removes) whatever it finds there the moment
        // it stamps a tab with a session id, so case 1's stale buffered
        // entries (a session whose tab is already gone, or that never
        // gets stamped because the open was itself cancelled/failed) sit
        // there harmlessly rather than silently losing case 2's data --
        // see that function's own doc comment for why it drains
        // unconditionally rather than only on a successful open.
        let target_id = arclain_app::ids::ArchiveSessionId::from_raw(archive_session_id);
        let tabs = self.signals.tabs.get();
        let matched = tabs
            .tabs()
            .iter()
            .find(|tab| tab.archive_session_id.get() == Some(target_id))
            .cloned();
        match matched {
            Some(tab) => tab.metadata.set(metadata),
            None => {
                let mut pending = self.signals.pending_session_metadata.lock().unwrap();
                // Bounded: see `MAX_PENDING_SESSION_METADATA`'s own doc
                // comment. `contains_key` first so a plugin re-reporting
                // metadata for a session it already buffered (still
                // waiting on its tab) can still update its own entry
                // even once the map is otherwise at capacity.
                if pending.len() >= MAX_PENDING_SESSION_METADATA
                    && !pending.contains_key(&target_id)
                {
                    tracing::warn!(
                        "[active_tab_bridge] pending_session_metadata is at its {} entry cap -- \
                         dropping metadata reported for session {target_id:?} instead of \
                         growing unbounded",
                        MAX_PENDING_SESSION_METADATA
                    );
                    return;
                }
                pending.insert(target_id, metadata);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for an unbounded-growth vector `set_session_metadata`
    /// introduced: it is a WASM plugin host function, so any plugin
    /// (buggy or hostile) can call it in a loop with made-up
    /// `archive_session_id`s that will never be stamped by a real tab --
    /// before the cap, every such call grew `pending_session_metadata`
    /// forever, since nothing ever drains an entry whose session never
    /// completes.
    #[test]
    fn set_session_metadata_does_not_grow_the_pending_buffer_past_its_cap() {
        let signals = AppSignals::new();
        let bridge = AppSignalsBridge::new(signals.clone());

        // No tab in a freshly-constructed AppSignals matches any of
        // these session ids, so every call takes the buffering branch.
        for raw_id in 0..(MAX_PENDING_SESSION_METADATA as u64 + 10) {
            bridge.set_session_metadata(raw_id, Some(serde_json::json!({"n": raw_id})));
        }

        let pending = signals.pending_session_metadata.lock().unwrap();
        assert!(
            pending.len() <= MAX_PENDING_SESSION_METADATA,
            "pending_session_metadata must never grow past its cap, got {} entries",
            pending.len()
        );
    }

    #[test]
    fn set_session_metadata_still_updates_an_already_buffered_entry_once_at_capacity() {
        let signals = AppSignals::new();
        let bridge = AppSignalsBridge::new(signals.clone());

        for raw_id in 0..MAX_PENDING_SESSION_METADATA as u64 {
            bridge.set_session_metadata(raw_id, Some(serde_json::json!({"n": raw_id})));
        }
        assert_eq!(
            signals.pending_session_metadata.lock().unwrap().len(),
            MAX_PENDING_SESSION_METADATA
        );

        // Re-reporting for a session already buffered (e.g. a plugin
        // updating its own guess before the tab is stamped) must still
        // go through even though the map is at capacity -- only brand
        // new session ids are subject to the cap.
        bridge.set_session_metadata(0, Some(serde_json::json!({"n": "updated"})));

        let pending = signals.pending_session_metadata.lock().unwrap();
        assert_eq!(pending.len(), MAX_PENDING_SESSION_METADATA);
        assert_eq!(
            pending.get(&arclain_app::ids::ArchiveSessionId::from_raw(0)),
            Some(&Some(serde_json::json!({"n": "updated"})))
        );
    }
}
