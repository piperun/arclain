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
//! `EventWorker`'s async fallback snapshots `metadata_signal()` at
//! the start of its async fetch, then writes to that snapshot when
//! the fetch completes. Because `Signal<T>` is `Arc`-internal, the
//! snapshot pins the write to *whichever tab was active when the
//! fetch started* — even if the user switches tabs while the HTTP
//! call is in flight, the metadata still lands on the original tab.

use arclain_signals::Signal;

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

    /// Signal handle for the active tab's metadata. Writes to the
    /// returned signal cause the active tab's `game_metadata` to
    /// update on the next frame. Callers that need write durability
    /// across tab switches should snapshot the returned `Signal`
    /// (cheap — internal `Arc` clone) before the write, so the
    /// write lands on the originally-targeted tab even if the user
    /// switches in the meantime.
    fn metadata_signal(&self) -> Signal<Option<serde_json::Value>>;

    /// Update the active tab's archive path. Used by
    /// `rename_archive`, which renames the underlying file and
    /// needs to reflect the new path in the tab state.
    fn set_archive_path(&self, path: Option<String>);
}
