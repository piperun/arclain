//! Per-tab whole-archive inventory, in the application facade's own
//! vocabulary.
//!
//! Where [`super::listing::TabListing`] holds the one *directory* a tab
//! is browsing, this holds the archive's whole entry tree -- the
//! [`ArchiveInventory`] `ArclainApp::list_all_entries` answers with --
//! for the consumers that genuinely need every entry at every depth: the
//! tree panel's folder set, the derived archive-info totals, drag-out's
//! recursive folder expansion, the plugin bridge's event snapshot, the
//! tab bar's entry count, and the command palette's file search.
//!
//! It replaces `TabState::entries`, the flat
//! `Vec<arclain_core::ArchiveEntry>` the operation bridge's own duplicate
//! `backend.list()` used to write. The rows here are the session's own --
//! `EntryId`s minted and validated server-side, folder rows carrying the
//! index's aggregates -- so the browser's whole-archive reads can no
//! longer drift from what the facade's id-consuming operations resolve
//! against.

use crate::core::utils::core_entry_from_dto;
use arclain_app::archive::{ArchiveEntryDto, ArchiveInventory};
use arclain_app::ids::ArchiveSessionId;
use std::sync::{Arc, OnceLock};

/// The shared "no archive open" projection, allocated once so per-frame
/// readers of an empty tab hand out the same `Arc` every time -- the
/// tree-projection cache keys on `Arc::ptr_eq`, and a fresh empty
/// allocation per frame would defeat it.
fn empty_legacy_rows() -> Arc<Vec<arclain_core::ArchiveEntry>> {
    static EMPTY: OnceLock<Arc<Vec<arclain_core::ArchiveEntry>>> = OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(Vec::new())).clone()
}

/// One adopted whole-archive answer: the facade's rows plus the derived
/// legacy projection, prepared *outside* any signal lock (see
/// [`Self::prepare`]) and swapped in as one `Arc`.
#[derive(Debug)]
pub struct AdoptedInventory {
    session_id: ArchiveSessionId,
    revision: u64,
    entries: Vec<ArchiveEntryDto>,
    /// TRANSITIONAL(4c): the same rows converted down to the pre-facade
    /// flat shape, memoized here (once per adoption, never per read) for
    /// the consumers that still speak it -- the legacy browser
    /// projections in `crate::core::operations::navigation_view`,
    /// drag-out's platform hand-off, and the plugin ABI's
    /// `EventContext.entries` (`Arc<Vec<arclain_core::ArchiveEntry>>` by
    /// contract in another crate, which is why this is an `Arc`: the
    /// bridge hands it out verbatim, zero-copy). A derived, immutable
    /// projection of the facade rows above -- never a store anything
    /// writes into; the encrypted-CRC backfill that used to mutate the
    /// flat list now writes into the session and arrives here as a
    /// fresh, higher-revision inventory.
    legacy_rows: Arc<Vec<arclain_core::ArchiveEntry>>,
}

impl AdoptedInventory {
    /// Converts one fetched [`ArchiveInventory`] into its adopted form,
    /// building the legacy projection row by row.
    ///
    /// `O(entries)` with a string allocation per row -- deliberately a
    /// free function a producer runs on its own task *before* taking the
    /// tab's signal lock, so a large archive's conversion never stalls a
    /// concurrent render-thread read of the same signal.
    pub fn prepare(inventory: ArchiveInventory) -> Arc<Self> {
        let legacy_rows = Arc::new(inventory.entries.iter().map(core_entry_from_dto).collect());
        Arc::new(Self {
            session_id: inventory.session_id,
            revision: inventory.revision,
            entries: inventory.entries,
            legacy_rows,
        })
    }
}

/// The whole-archive rows a tab holds for its open session, or nothing
/// before the first successful fetch.
///
/// The adoption guards mirror [`super::listing::TabListing`]'s
/// data-integrity pair -- session and revision -- but deliberately not
/// its request-generation identity: an inventory carries no
/// per-directory request to misattribute and no status axis a stale
/// reply could corrupt, so "same session, revision not older than held"
/// is already the whole correctness condition. Two racing refreshes
/// converge on the higher revision regardless of reply order, and a
/// same-revision refetch re-seats identical rows.
// No `PartialEq`: `arclain_core::ArchiveEntry` (inside the legacy
// projection) has none, and nothing calls `set_if_changed` on the
// inventory signal -- producers always `update` it through the guarded
// `adopt`.
#[derive(Clone, Debug, Default)]
pub struct TabInventory {
    /// The archive session these rows must come from, `None` before the
    /// tab has an archive open. Load-bearing exactly as on `TabListing`:
    /// the rows carry session-scoped `EntryId`s, so a stale session's
    /// inventory must never seat under a newer binding.
    session: Option<ArchiveSessionId>,
    rows: Option<Arc<AdoptedInventory>>,
}

impl TabInventory {
    /// A fresh, row-less inventory bound to `session` -- what a tab
    /// holds the moment an archive open completes (rows follow when the
    /// first fetch answers), and (with `None`) what a tab with no
    /// archive open holds.
    pub fn for_session(session: Option<ArchiveSessionId>) -> Self {
        Self {
            session,
            rows: None,
        }
    }

    /// The archive session this inventory belongs to.
    pub fn session(&self) -> Option<ArchiveSessionId> {
        self.session
    }

    /// The revision the held rows describe, `None` while no fetch has
    /// answered yet.
    pub fn revision(&self) -> Option<u64> {
        self.rows.as_ref().map(|rows| rows.revision)
    }

    /// Every entry of the archive at every depth, in the facade's
    /// depth-first tree order -- empty until a fetch answers.
    pub fn entries(&self) -> &[ArchiveEntryDto] {
        match &self.rows {
            Some(rows) => &rows.entries,
            None => &[],
        }
    }

    /// How many entries the archive holds (files plus directories,
    /// synthesized ancestors included -- the same definition
    /// `ArchiveSnapshot::entry_count` reports). Zero until a fetch
    /// answers.
    pub fn entry_count(&self) -> usize {
        self.rows.as_ref().map_or(0, |rows| rows.entries.len())
    }

    /// TRANSITIONAL(4c): the held rows in the pre-facade flat shape --
    /// see [`AdoptedInventory::legacy_rows`]. The shared empty list when
    /// no rows are held, so an empty tab's per-frame readers keep a
    /// stable `Arc` identity.
    pub fn legacy_rows(&self) -> Arc<Vec<arclain_core::ArchiveEntry>> {
        match &self.rows {
            Some(rows) => rows.legacy_rows.clone(),
            None => empty_legacy_rows(),
        }
    }

    /// Seats `prepared` as this tab's whole-archive rows.
    ///
    /// Refuses (reporting `false`) an answer that cannot describe what
    /// the tab holds now: one from a session this inventory does not
    /// belong to (a late reply for the archive the tab held before), or
    /// one older than the rows already held (a refetch overtaken by a
    /// newer one that already answered). An equal revision re-seats:
    /// the rows are identical by construction, and refusing them would
    /// make a harmless refetch look like a failure.
    ///
    /// `O(1)` under the caller's signal lock -- the expensive half
    /// happened in [`AdoptedInventory::prepare`].
    pub fn adopt(&mut self, prepared: Arc<AdoptedInventory>) -> bool {
        if self.session != Some(prepared.session_id) {
            return false;
        }
        if let Some(held) = &self.rows {
            if prepared.revision < held.revision {
                return false;
            }
        }
        self.rows = Some(prepared);
        true
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
