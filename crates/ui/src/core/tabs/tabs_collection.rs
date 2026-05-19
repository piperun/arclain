//! Ordered collection of tabs with an active TabId.

use super::tab_state::TabState;
use super::TabId;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Maximum number of recently-closed tab paths kept for Ctrl+Shift+T
/// reopen. Oldest entries fall off when the buffer is full.
const RECENTLY_CLOSED_LIMIT: usize = 10;

#[derive(Clone)]
pub struct TabsCollection {
    tabs: Vec<Arc<TabState>>,
    active: TabId,
    next_id: u64,
    /// LIFO ring buffer of recently-closed archive paths. Only tabs
    /// with a loaded archive are remembered — closing an empty
    /// placeholder tab does not push anything (would just reopen
    /// another empty placeholder, which is useless).
    recently_closed: VecDeque<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseResult {
    Closed,
    BlockedByInFlight { count: usize },
    NotFound,
}

impl TabsCollection {
    /// Create a collection with one empty tab (id = TabId(1)). Matches
    /// the Phase 2a single-tab assumption.
    pub fn new() -> Self {
        let first = Arc::new(TabState::new(TabId(1)));
        Self {
            tabs: vec![first],
            active: TabId(1),
            next_id: 2,
            recently_closed: VecDeque::new(),
        }
    }

    pub fn active(&self) -> &Arc<TabState> {
        self.tabs
            .iter()
            .find(|t| t.id == self.active)
            .expect("active TabId must always reference an existing tab")
    }

    pub fn get(&self, id: TabId) -> Option<&Arc<TabState>> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tabs(&self) -> &[Arc<TabState>] {
        &self.tabs
    }

    pub fn active_id(&self) -> TabId {
        self.active
    }

    pub fn open(&mut self, archive_path: Option<PathBuf>) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        let tab = Arc::new(TabState::new(id));
        if let Some(path) = archive_path {
            tab.archive_path.set(Some(path));
        }
        self.tabs.push(tab);
        self.active = id;
        id
    }

    /// Replace the active tab's state with a fresh TabState (same id)
    /// holding the new archive path.
    ///
    /// **Caveat:** if a background op spawned against the old tab state
    /// is still in flight, it holds its own `Arc<TabState>` clone. The
    /// op will continue and write its results into the now-replaced
    /// state object, which is no longer in the collection — the results
    /// are silently discarded. Callers should either avoid replace_active
    /// while ops are in flight, or wire a per-tab cancellation token
    /// that the ops honor (Phase 2c).
    pub fn replace_active(&mut self, archive_path: PathBuf) {
        let active_id = self.active;
        let idx = self
            .tabs
            .iter()
            .position(|t| t.id == active_id)
            .expect("active TabId must reference an existing tab");
        let new_tab = Arc::new(TabState::new(active_id));
        new_tab.archive_path.set(Some(archive_path));
        self.tabs[idx] = new_tab;
    }

    pub fn close(&mut self, id: TabId) -> CloseResult {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return CloseResult::NotFound;
        };
        let in_flight = tab.in_flight_ops.load(Ordering::SeqCst);
        if in_flight > 0 {
            return CloseResult::BlockedByInFlight { count: in_flight };
        }
        self.remember_closed(id);
        self.remove(id);
        CloseResult::Closed
    }

    /// Force-close without the in-flight check. Used after the user
    /// confirms the close-tab modal — they have explicitly accepted that
    /// in-flight ops will be cancelled.
    ///
    /// Fires the tab's `tab_cancel` flag BEFORE removing it from the
    /// collection so any background op still holding an `Arc<TabState>`
    /// clone can observe the cancellation on its next periodic check.
    /// Per the v1 contract this is best-effort: ops that don't yet check
    /// the flag continue against the captured Arc until completion.
    pub fn force_close(&mut self, id: TabId) {
        if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
            tab.tab_cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.remember_closed(id);
        self.remove(id);
    }

    /// Push the tab's archive path onto the recently-closed buffer so
    /// Ctrl+Shift+T can resurrect it. Empty placeholder tabs are not
    /// remembered. Oldest entries fall off when the buffer is full.
    fn remember_closed(&mut self, id: TabId) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else {
            return;
        };
        let Some(path) = tab.archive_path.get() else {
            return;
        };
        self.recently_closed.push_back(path);
        while self.recently_closed.len() > RECENTLY_CLOSED_LIMIT {
            self.recently_closed.pop_front();
        }
    }

    /// Reopen the most recently closed tab (LIFO). Returns the new
    /// tab's id, or None if nothing was remembered. The reopened tab
    /// is a fresh empty tab containing only the archive path — the
    /// caller is responsible for triggering the actual archive load
    /// (the open-archive flow in dialog_handler does this when it
    /// notices an active tab has an unloaded archive_path).
    pub fn reopen_last_closed(&mut self) -> Option<(TabId, PathBuf)> {
        let path = self.recently_closed.pop_back()?;
        let id = self.open(Some(path.clone()));
        Some((id, path))
    }

    /// True if `reopen_last_closed` would return Some.
    pub fn has_recently_closed(&self) -> bool {
        !self.recently_closed.is_empty()
    }

    fn remove(&mut self, id: TabId) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            // Never have zero tabs — spawn a replacement empty tab.
            let new_id = TabId(self.next_id);
            self.next_id += 1;
            self.tabs.push(Arc::new(TabState::new(new_id)));
            self.active = new_id;
        } else if self.active == id {
            // Closed tab was active; activate the neighbour (prefer right).
            let new_active_idx = idx.min(self.tabs.len() - 1);
            self.active = self.tabs[new_active_idx].id;
        }
    }

    pub fn switch_to(&mut self, id: TabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active = id;
        }
    }

    pub fn reorder(&mut self, from_idx: usize, to_idx: usize) {
        if from_idx >= self.tabs.len() || to_idx >= self.tabs.len() || from_idx == to_idx {
            return;
        }
        let tab = self.tabs.remove(from_idx);
        self.tabs.insert(to_idx, tab);
    }
}

impl TabsCollection {
    /// Expose `next_id` for persistence snapshots so restored sessions
    /// keep generating monotonically without colliding with restored
    /// TabIds.
    pub fn peek_next_id(&self) -> u64 {
        self.next_id
    }

    /// Rebuild a TabsCollection from a persistence snapshot. Handles
    /// edge cases:
    /// - Empty snapshot.tabs → seed a single empty placeholder so
    ///   TabsCollection's "never zero tabs" invariant holds.
    /// - Snapshot.active not in snapshot.tabs → fall back to first tab.
    pub fn from_snapshot(snap: super::persistence::TabsSnapshot) -> Self {
        let tabs: Vec<Arc<TabState>> = snap
            .tabs
            .iter()
            .map(|tr| {
                let tab = TabState::new(tr.id);
                if let Some(path) = &tr.archive_path {
                    tab.archive_path.set(Some(path.clone()));
                }
                Arc::new(tab)
            })
            .collect();

        if tabs.is_empty() {
            // Empty snapshot → reset to a fresh single-tab collection.
            // Use snap.next_id to keep id sequencing monotonic.
            let id = TabId(snap.next_id.max(1));
            let fresh = Arc::new(TabState::new(id));
            return Self {
                tabs: vec![fresh],
                active: id,
                next_id: snap.next_id.saturating_add(1).max(2),
                recently_closed: VecDeque::new(),
            };
        }

        let active = if tabs.iter().any(|t| t.id == snap.active) {
            snap.active
        } else {
            tabs[0].id
        };

        Self {
            tabs,
            active,
            next_id: snap.next_id,
            recently_closed: VecDeque::new(),
        }
    }
}

impl Default for TabsCollection {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TabsCollection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabsCollection")
            .field("active", &self.active)
            .field("tabs_len", &self.tabs.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

#[cfg(test)]
#[path = "tabs_collection_tests.rs"]
mod tests;
