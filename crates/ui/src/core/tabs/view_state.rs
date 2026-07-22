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
use std::collections::HashSet;
use std::ops::Deref;
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
    visible_search: String,
    visible: Vec<usize>,
    visible_generation: u64,
    selected_revision: u64,
    selected_for: u64,
    selected: Vec<usize>,
    visible_selection_generation: u64,
    visible_selection_for: u64,
    visible_selected_count: usize,
    #[cfg(test)]
    rebuilds: usize,
    #[cfg(test)]
    normalizations: usize,
    #[cfg(test)]
    selection_work: SelectionWorkCounts,
}

pub struct BrowserRenderProjection<'a> {
    pub visible_indices: &'a [usize],
    pub selected_indices: &'a [usize],
    pub visible_selected_count: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SelectionWorkCounts {
    pub selected_rebuilds: usize,
    pub selected_entry_visits: usize,
    pub visible_selection_rebuilds: usize,
    pub visible_entry_visits: usize,
}

impl BrowserProjectionCache {
    pub fn visible_indices(
        &mut self,
        snapshot: &BrowserEntriesSnapshot,
        sort: SortState,
        search: &str,
    ) -> &[usize] {
        self.rebuild_visible_if_needed(snapshot, sort, search);
        &self.visible
    }

    pub fn render_projection(
        &mut self,
        snapshot: &BrowserEntriesSnapshot,
        sort: SortState,
        search: &str,
        selection: &RevisionedSelection,
    ) -> BrowserRenderProjection<'_> {
        self.rebuild_visible_if_needed(snapshot, sort, search);
        let selection_revision = selection.revision();

        if self.selected_revision != snapshot.revision || self.selected_for != selection_revision {
            self.selected.clear();
            self.selected.extend(
                snapshot
                    .entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| selection.contains(&entry.path).then_some(index)),
            );
            self.selected_revision = snapshot.revision;
            self.selected_for = selection_revision;
            #[cfg(test)]
            {
                self.selection_work.selected_rebuilds += 1;
                self.selection_work.selected_entry_visits += snapshot.entries.len();
            }
        }

        if self.visible_selection_generation != self.visible_generation
            || self.visible_selection_for != selection_revision
        {
            self.visible_selected_count = self
                .visible
                .iter()
                .filter(|index| selection.contains(&snapshot.entries[**index].path))
                .count();
            self.visible_selection_generation = self.visible_generation;
            self.visible_selection_for = selection_revision;
            #[cfg(test)]
            {
                self.selection_work.visible_selection_rebuilds += 1;
                self.selection_work.visible_entry_visits += self.visible.len();
            }
        }

        BrowserRenderProjection {
            visible_indices: &self.visible,
            selected_indices: &self.selected,
            visible_selected_count: self.visible_selected_count,
        }
    }

    fn rebuild_visible_if_needed(
        &mut self,
        snapshot: &BrowserEntriesSnapshot,
        sort: SortState,
        search: &str,
    ) {
        if self.sorted_revision != snapshot.revision || self.sorted_for != sort {
            self.sorted = (0..snapshot.entries.len()).collect();
            sort_entry_indices(&snapshot.entries, &mut self.sorted, sort);
            self.sorted_revision = snapshot.revision;
            self.sorted_for = sort;
        }

        let search = search.trim();
        if self.visible_revision != snapshot.revision
            || self.visible_for != sort
            || self.visible_search != search
        {
            #[cfg(test)]
            {
                self.normalizations += 1;
            }
            let filter = search.to_lowercase();
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
            self.visible_search.clear();
            self.visible_search.push_str(search);
            self.visible_generation = self.visible_generation.wrapping_add(1).max(1);
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn rebuild_count(&self) -> usize {
        self.rebuilds
    }

    #[cfg(test)]
    pub(crate) fn normalization_count(&self) -> usize {
        self.normalizations
    }

    #[cfg(test)]
    pub(crate) fn selection_work_counts(&self) -> SelectionWorkCounts {
        self.selection_work
    }
}

/// Renderer-owned selection with an explicit invalidation revision.
///
/// Only immutable `HashSet` access is exposed. Every mutating operation
/// advances `revision`, so projection caches never need to hash or scan the
/// complete selection merely to detect a change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RevisionedSelection {
    paths: Arc<HashSet<String>>,
    revision: u64,
}

impl RevisionedSelection {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }

    pub fn insert(&mut self, path: String) -> bool {
        if self.paths.contains(&path) {
            return false;
        }
        Arc::make_mut(&mut self.paths).insert(path);
        self.bump_revision();
        true
    }

    pub fn remove(&mut self, path: &str) -> bool {
        if !self.paths.contains(path) {
            return false;
        }
        Arc::make_mut(&mut self.paths).remove(path);
        self.bump_revision();
        true
    }

    pub fn clear(&mut self) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        Arc::make_mut(&mut self.paths).clear();
        self.bump_revision();
        true
    }

    pub fn extend(&mut self, paths: impl IntoIterator<Item = String>) -> bool {
        let additions = paths
            .into_iter()
            .filter(|path| !self.paths.contains(path))
            .collect::<Vec<_>>();
        if additions.is_empty() {
            return false;
        }
        Arc::make_mut(&mut self.paths).extend(additions);
        self.bump_revision();
        true
    }
}

impl Deref for RevisionedSelection {
    type Target = HashSet<String>;

    fn deref(&self) -> &Self::Target {
        &self.paths
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserViewState {
    /// Renderer-owned paths selected in the current browser projection.
    pub selection: RevisionedSelection,
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

        view.update(|state| {
            state.selection.clear();
            state.selection.insert("worker-new".to_string());
        });

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

    #[test]
    fn settled_nonempty_filter_is_normalized_only_once() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(vec![entry("Alpha"), entry("beta")]);
        let mut cache = BrowserProjectionCache::default();

        assert_eq!(
            cache.visible_indices(&snapshot, SortState::default(), " ALP "),
            &[0]
        );
        assert_eq!(cache.normalization_count(), 1);

        assert_eq!(
            cache.visible_indices(&snapshot, SortState::default(), "ALP"),
            &[0]
        );
        assert_eq!(
            cache.normalization_count(),
            1,
            "settled frame normalized and allocated the unchanged filter"
        );
    }

    #[test]
    fn settled_list_projection_does_not_rescan_ten_thousand_visible_entries() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(
            (0..10_000)
                .rev()
                .map(|index| entry(&format!("entry-{index:05}")))
                .collect(),
        );
        let selection = RevisionedSelection::default();
        let mut cache = BrowserProjectionCache::default();

        let first_visible_ptr = {
            let projection =
                cache.render_projection(&snapshot, SortState::default(), "", &selection);
            assert_eq!(projection.visible_indices.len(), 10_000);
            assert_eq!(projection.visible_selected_count, 0);
            projection.visible_indices.as_ptr()
        };
        let first_work = cache.selection_work_counts();
        assert_eq!(first_work.selected_rebuilds, 1);
        assert_eq!(first_work.selected_entry_visits, 10_000);
        assert_eq!(first_work.visible_selection_rebuilds, 1);
        assert_eq!(first_work.visible_entry_visits, 10_000);

        let second_visible_ptr = {
            let projection =
                cache.render_projection(&snapshot, SortState::default(), "", &selection);
            projection.visible_indices.as_ptr()
        };

        assert_eq!(first_visible_ptr, second_visible_ptr);
        assert_eq!(cache.selection_work_counts(), first_work);
    }

    #[test]
    fn settled_properties_projection_reuses_selected_indices() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(
            (0..10_000)
                .map(|index| entry(&format!("entry-{index:05}")))
                .collect(),
        );
        let mut selection = RevisionedSelection::default();
        assert!(selection.insert("entry-05000".to_string()));
        let mut cache = BrowserProjectionCache::default();

        let first_selected_ptr = {
            let projection =
                cache.render_projection(&snapshot, SortState::default(), "", &selection);
            assert_eq!(projection.selected_indices, &[5_000]);
            projection.selected_indices.as_ptr()
        };
        let first_work = cache.selection_work_counts();
        assert_eq!(first_work.selected_rebuilds, 1);
        assert_eq!(first_work.selected_entry_visits, 10_000);
        assert_eq!(first_work.visible_selection_rebuilds, 1);
        assert_eq!(first_work.visible_entry_visits, 10_000);

        let second_selected_ptr = {
            let projection =
                cache.render_projection(&snapshot, SortState::default(), "", &selection);
            projection.selected_indices.as_ptr()
        };

        assert_eq!(first_selected_ptr, second_selected_ptr);
        assert_eq!(cache.selection_work_counts(), first_work);
    }

    #[test]
    fn selection_and_filter_revisions_invalidate_only_their_dependent_work() {
        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(vec![entry("visible"), entry("hidden")]);
        let mut selection = RevisionedSelection::default();
        assert!(selection.insert("hidden".to_string()));
        let mut cache = BrowserProjectionCache::default();

        let projection =
            cache.render_projection(&snapshot, SortState::default(), "visible", &selection);
        assert_eq!(projection.visible_indices, &[0]);
        assert_eq!(projection.selected_indices, &[1]);
        assert_eq!(projection.visible_selected_count, 0);
        let initial = cache.selection_work_counts();

        assert!(selection.insert("visible".to_string()));
        let projection =
            cache.render_projection(&snapshot, SortState::default(), "visible", &selection);
        assert_eq!(projection.visible_selected_count, 1);
        assert_eq!(projection.selected_indices, &[0, 1]);
        let after_selection = cache.selection_work_counts();
        assert_eq!(
            after_selection.selected_rebuilds,
            initial.selected_rebuilds + 1
        );
        assert_eq!(
            after_selection.visible_selection_rebuilds,
            initial.visible_selection_rebuilds + 1
        );

        let projection =
            cache.render_projection(&snapshot, SortState::default(), "hidden", &selection);
        assert_eq!(projection.visible_indices, &[1]);
        assert_eq!(projection.visible_selected_count, 1);
        let after_filter = cache.selection_work_counts();
        assert_eq!(
            after_filter.selected_rebuilds,
            after_selection.selected_rebuilds
        );
        assert_eq!(
            after_filter.visible_selection_rebuilds,
            after_selection.visible_selection_rebuilds + 1
        );
    }

    #[test]
    fn revisioned_selection_clone_shares_storage_until_mutation() {
        let mut original = RevisionedSelection::default();
        assert!(original.extend((0..10_000).map(|index| format!("entry-{index:05}"))));
        let mut cloned = original.clone();

        assert!(
            std::ptr::eq(&*original, &*cloned),
            "cloning renderer state copied the complete selected-path set"
        );

        let cloned_revision = cloned.revision();
        assert!(!cloned.insert("entry-00000".to_string()));
        assert_eq!(cloned.revision(), cloned_revision);
        assert!(
            std::ptr::eq(&*original, &*cloned),
            "a no-op selection mutation detached shared storage"
        );

        assert!(cloned.insert("new-entry".to_string()));
        assert_ne!(cloned.revision(), cloned_revision);
        assert!(!std::ptr::eq(&*original, &*cloned));
        assert!(!original.contains("new-entry"));
    }
}
