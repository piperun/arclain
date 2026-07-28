//! Bridge between the plugin system and the host's per-tab signal tree.
//!
//! Plugins read "what archive is currently active?" / write "this is
//! the metadata for the current archive" through this trait. The host
//! (arclain_ui) implements it by resolving through `AppSignals` at
//! call time, so the answer always reflects the *currently active
//! tab* — no stale handles, no per-frame sync block.
//!
//! ## Why a bridge instead of held state?
//!
//! Pre-bridge, the manager held a `Signal<Option<serde_json::Value>>`
//! handle captured once at init from the placeholder tab's
//! `metadata` signal. Every subsequent tab op (`replace_active`,
//! `col.open`, `restore_tabs_on_launch`) created a fresh `TabState`
//! with brand-new per-tab signals, orphaning the held handle. The
//! plugin emitted metadata to the orphan, the active tab's signal
//! stayed `None`, and the UI never showed the result.
//!
//! Adding a per-frame sync that re-pushed the active tab's signals
//! into the manager worked but was a band-aid — every new per-tab
//! signal a plugin needs would have required updating the sync
//! block. The bridge resolves at call time instead, which:
//!
//! - eliminates the stale-handle class of bug entirely
//! - keeps all "what's the active tab right now?" knowledge in one
//!   place (the host's impl), so adding a new per-tab signal is one
//!   trait method + one impl method
//! - costs only an `Arc` clone per call (signals are `Arc`-internal)
//!
//! ## In-flight write semantics
//!
//! `EventWorker`'s async fallback snapshots [`ActiveTabBridge::
//! active_archive_session_id`] at the start of its async fetch, then
//! writes to that snapshotted session id (via
//! [`ActiveTabBridge::set_session_metadata`]) when the fetch completes.
//! Resolving by session id rather than "whichever tab is active *now*"
//! is what pins the write to *whichever tab was active when the fetch
//! started* — even if the user switches tabs while the HTTP call is in
//! flight, the metadata still lands on the original tab.
//!
//! ## Decoupled from UI signal types
//!
//! This trait used to expose a `metadata_signal() -> arclain_signals::
//! Signal<Option<serde_json::Value>>` method: a panel-driven (non-event)
//! `emit_metadata` call wrote directly into whatever signal that method
//! handed back. That was the one place a UI-toolkit-adjacent type
//! (`arclain_signals::Signal`) appeared in this crate's public API.
//! [`Self::active_archive_session_id`] replaces it: the panel-driven
//! path now resolves "which session is active right now" the same way
//! the event-driven path already resolved "which session did this event
//! fire for", and both funnel through the one write sink,
//! [`Self::set_session_metadata`]. `arclain_plugins` no longer depends on
//! `arclain_signals` at all (not even as a dev-dependency): every test
//! double that previously used `Signal` as a convenient interior-mutable
//! cell now uses a plain `Mutex` instead.

/// Host-side bridge giving the plugin system a live view of the
/// currently active tab's per-tab signals.
pub trait ActiveTabBridge: Send + Sync {
    /// Filesystem path of the archive currently open in the active
    /// tab, or `None` if no archive is open.
    fn archive_path(&self) -> Option<String>;

    /// Password for the active tab's archive (if the user has
    /// unlocked it), or `None`.
    fn current_password(&self) -> Option<String>;

    /// In-archive paths the host has already listed for the active
    /// tab. Returned in archive order. Empty if no archive is open,
    /// or if the archive is encrypted and not yet unlocked (no
    /// listing has been produced).
    ///
    /// The host populates this when `list_archive` runs at open
    /// time — plugins reading it pay only an `Arc` clone + a per-
    /// entry `String` clone, never a backend re-list. Use this in
    /// preference to making the plugin call its own listing
    /// function (which would round-trip through the archive
    /// backend and, for 7z, spawn a subprocess each time).
    fn archive_entries(&self) -> Vec<String>;

    /// Number of entries in the active archive without cloning their paths.
    fn archive_entry_count(&self) -> usize {
        self.archive_entries().len()
    }

    /// Clone only the requested entry-path page. Implementations backed by an
    /// `Arc<Vec<_>>` should override this to avoid cloning the complete list.
    fn archive_entries_page(&self, offset: usize, limit: usize) -> Vec<String> {
        self.archive_entries()
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect()
    }

    /// The opaque archive session id (an application-facade
    /// `ArchiveSessionId::into_raw()` value) of whichever tab is
    /// currently active, or `None` if the active tab has no archive
    /// open. Read by the non-event-context path of `emit_metadata` to
    /// resolve which session a *user-initiated* emit (a plugin panel
    /// action with no event context) belongs to — "whichever tab I'm
    /// looking at right now" is the correct semantic there — before
    /// writing through [`Self::set_session_metadata`]. An event-context
    /// emit (one fired while dispatching a queued
    /// `PluginEvent::OnArchiveOpen`) never calls this: it already has
    /// its own originating session id and routes through
    /// [`Self::set_session_metadata`] directly, since the tab that
    /// requested the event may no longer be the active one by the time
    /// it's processed.
    fn active_archive_session_id(&self) -> Option<u64>;

    /// Writes `metadata` for the tab (if any) currently holding
    /// `archive_session_id` open — resolved by session id, independent
    /// of which tab is currently active. Used by the event-context path
    /// of `emit_metadata`, replacing the previous approach of capturing
    /// a UI `Signal` handle directly on `PluginEvent::OnArchiveOpen` (see
    /// that type's doc comment): the application layer that fires the
    /// event only has an opaque session id to hand over, never a UI
    /// signal. A no-op if no tab currently holds that session (it was
    /// closed, or the id does not correspond to any tab this bridge
    /// knows about) — the write is simply lost, matching the pre-
    /// existing behavior for a plugin event whose originating tab was
    /// already closed by the time the worker processed it.
    fn set_session_metadata(&self, archive_session_id: u64, metadata: Option<serde_json::Value>);

    /// Update the active tab's archive path. Used by
    /// `rename_archive`, which renames the underlying file and
    /// needs to reflect the new path in the tab state.
    fn set_archive_path(&self, path: Option<String>);
}
