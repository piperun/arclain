use super::domain::Action;
use super::presentation::BrowserController;
use crate::core::tabs::view_state::{ArchiveTreeProjectionCache, BrowserProjectionCache};
use crate::core::tabs::TabId;
use crate::shared::SharedState;
use eframe::egui;
use std::collections::{HashMap, HashSet};

pub struct ArchiveBrowser {
    pub controller: BrowserController,
    projections: HashMap<TabId, ArchiveTabProjectionCache>,
}

#[derive(Default)]
struct ArchiveTabProjectionCache {
    files: BrowserProjectionCache,
    tree: ArchiveTreeProjectionCache,
}

impl ArchiveBrowser {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            controller: BrowserController::new(),
            projections: HashMap::new(),
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> Action {
        let tabs = shared.signals().tabs.get();
        let live: HashSet<TabId> = tabs.tabs().iter().map(|tab| tab.id).collect();
        self.projections.retain(|id, _| live.contains(id));
        let active = tabs.active().clone();
        let projection = self.projections.entry(active.id).or_default();
        super::presentation::views::browser_page::render_archive_browser(
            ctx,
            shared,
            &active,
            &mut projection.files,
            &mut projection.tree,
        )
    }

    #[doc(hidden)]
    pub fn tree_projection_rebuild_count(&self, tab_id: TabId) -> usize {
        self.projections
            .get(&tab_id)
            .map(|projection| projection.tree.rebuild_count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tabs::view_state::BrowserEntriesSnapshot;
    use crate::core::tabs::TabId;
    use crate::shared::models::file_entry::{FileEntry, SortState};
    use std::collections::HashSet;

    #[test]
    fn archive_browser_keeps_projection_for_the_active_tab() {
        let tab_id = TabId(7);
        let mut browser = ArchiveBrowser {
            controller: BrowserController::new(),
            projections: Default::default(),
        };
        browser
            .projections
            .insert(tab_id, ArchiveTabProjectionCache::default());

        let mut snapshot = BrowserEntriesSnapshot::default();
        snapshot.replace(vec![FileEntry {
            name: "entry".to_string(),
            path: "entry".to_string(),
            archive_path: "entry".to_string(),
            size: "0 B".to_string(),
            compressed: "0 B".to_string(),
            ratio: "0%".to_string(),
            modified: String::new(),
            crc32: String::new(),
            encrypted: false,
            is_folder: false,
        }]);
        let sort = SortState::default();

        let cache = &mut browser.projections.get_mut(&tab_id).unwrap().files;
        cache.visible_indices(&snapshot, sort, "");
        cache.visible_indices(&snapshot, sort, "");
        assert_eq!(cache.rebuild_count(), 1);

        let live = HashSet::from([TabId(8)]);
        browser.projections.retain(|id, _| live.contains(id));
        assert!(!browser.projections.contains_key(&tab_id));
    }
}
