//! Per-tab browser-view state.
//!
//! `BrowserViewState` lives in `core/tabs/` because it's a property of
//! the **tab** — every tab in the multi-archive UI owns one, and
//! `TabState` is the source of truth. It was previously misfiled under
//! `features/archive_browser/domain/types.rs`, which forced
//! `core/tabs/tab_state.rs` to import from `features/` (violating the
//! `core/ ⊥ features/` invariant — see
//! docs/audits/2026-05-19-dependencies.md §2 + §5 medium #9).
//!
//! All fields are `shared/` types (file entries, sort state, toolbar
//! state, tree panel state) — none are archive_browser-specific — so
//! the relocation is a clean move with no field-level split.
//! `features/archive_browser` continues to consume the type, but now
//! through its core-owned home.

use crate::shared::components::toolbar::ToolbarState;
use crate::shared::components::tree_panel::TreePanelState;
use crate::shared::models::file_entry::{sort_entry_indices, FileEntry, SortState};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct BrowserEntriesSnapshot {
    pub revision: u64,
    pub entries: Arc<[FileEntry]>,
}

impl Default for BrowserEntriesSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            entries: Arc::from(Vec::new()),
        }
    }
}

impl BrowserEntriesSnapshot {
    pub fn replace(&mut self, entries: Vec<FileEntry>) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.entries = Arc::from(entries);
    }
}

/// Renderer-owned sorted and filtered index projections for one tab.
#[derive(Default)]
pub struct BrowserProjectionCache {
    sorted_revision: u64,
    sorted_for: SortState,
    sorted: Vec<usize>,
    visible_revision: u64,
    visible_for: SortState,
    visible_filter: String,
    visible: Vec<usize>,
    #[cfg(test)]
    rebuilds: usize,
}

impl BrowserProjectionCache {
    pub fn visible_indices(
        &mut self,
        snapshot: &BrowserEntriesSnapshot,
        sort: SortState,
        search: &str,
    ) -> &[usize] {
        if self.sorted_revision != snapshot.revision || self.sorted_for != sort {
            self.sorted = (0..snapshot.entries.len()).collect();
            sort_entry_indices(&snapshot.entries, &mut self.sorted, sort);
            self.sorted_revision = snapshot.revision;
            self.sorted_for = sort;
        }

        let filter = search.trim().to_lowercase();
        if self.visible_revision != snapshot.revision
            || self.visible_for != sort
            || self.visible_filter != filter
        {
            self.visible.clear();
            self.visible
                .extend(self.sorted.iter().copied().filter(|index| {
                    filter.is_empty()
                        || snapshot.entries[*index]
                            .name
                            .to_lowercase()
                            .contains(&filter)
                }));
            self.visible_revision = snapshot.revision;
            self.visible_for = sort;
            self.visible_filter = filter;
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
        }
        &self.visible
    }

    #[cfg(test)]
    pub(crate) fn rebuild_count(&self) -> usize {
        self.rebuilds
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserViewState {
    /// Renderer-owned paths selected in the current browser projection.
    pub selection: std::collections::HashSet<String>,
    // NOTE: current_path moved to NavigationState signal pre-relocation
    //       for single source of truth; that history is preserved here.
    pub toolbar_state: ToolbarState,
    pub sort_state: SortState,
    pub tree_state: TreePanelState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_signals::Signal;
    use std::collections::HashSet;
    use std::sync::Arc;

    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: name.to_string(),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        }
    }

    #[test]
    fn browser_entry_snapshot_clone_shares_the_entry_allocation() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(vec![entry("a"), entry("b")]);
        let signal = Signal::new(snapshot);
        let first = signal.get();
        let second = signal.get();

        assert!(Arc::ptr_eq(&first.entries, &second.entries));
        assert_eq!(first.revision, second.revision);
    }

    #[test]
    fn renderer_state_write_cannot_overwrite_same_length_worker_entries() {
        let entries = Signal::new(BrowserEntriesSnapshot::default());
        let view = Signal::new(BrowserViewState::default());
        entries.update(|snapshot| snapshot.replace(vec![entry("worker-new")]));
        let revision = entries.get().revision;

        view.update(|state| state.selection = HashSet::from(["worker-new".to_string()]));

        let current = entries.get();
        assert_eq!(current.entries[0].name, "worker-new");
        assert_eq!(current.revision, revision);
    }

    #[test]
    fn projection_cache_rebuilds_only_for_revision_sort_or_filter_change() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(vec![entry("b"), entry("a")]);
        let mut cache = BrowserProjectionCache::default();
        let ascending = SortState::default();

        assert_eq!(cache.visible_indices(&snapshot, ascending, ""), &[1, 0]);
        assert_eq!(cache.rebuild_count(), 1);
        assert_eq!(cache.visible_indices(&snapshot, ascending, ""), &[1, 0]);
        assert_eq!(cache.rebuild_count(), 1, "idle frame rebuilt projection");

        let descending = SortState {
            ascending: false,
            ..ascending
        };
        assert_eq!(cache.visible_indices(&snapshot, descending, ""), &[0, 1]);
        assert_eq!(cache.rebuild_count(), 2);
        assert_eq!(cache.visible_indices(&snapshot, descending, "a"), &[1]);
        assert_eq!(cache.rebuild_count(), 3);
    }
}
