//! Ordered collection of tabs with an active TabId.

use super::tab_state::TabState;
use super::TabId;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Clone)]
pub struct TabsCollection {
    tabs: Vec<Arc<TabState>>,
    active: TabId,
    next_id: u64,
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
        self.remove(id);
        CloseResult::Closed
    }

    /// Force-close without the in-flight check. Used after the user
    /// confirms the close-tab modal — they have explicitly accepted
    /// that in-flight ops will be cancelled.
    pub fn force_close(&mut self, id: TabId) {
        self.remove(id);
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
