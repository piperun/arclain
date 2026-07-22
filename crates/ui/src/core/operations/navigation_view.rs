//! Navigation view utilities
//!
//! Helper functions for updating view state after navigation changes.

use crate::core::signals::AppSignals;
use crate::core::tabs::{TabId, TabState};
use crate::core::utils::convert_to_file_entry;
use std::sync::Arc;
use tracing::info;

/// Publish the active tab's worker-owned browser snapshot for its current path.
pub fn refresh_view_entries(signals: &AppSignals) {
    let tab = signals.tabs.get().active().clone();
    refresh_view_entries_for(&tab);
}

/// Refresh browser entries for a specific tab. Used when an archive is
/// loaded into a non-active tab (e.g. multi-file drop opens several
/// tabs but only the last is active; each must populate its own
/// browser snapshot so a future switch shows the file list immediately).
pub fn refresh_view_entries_for_tab(signals: &AppSignals, tab_id: TabId) {
    let Some(tab) = signals.tabs.get().get(tab_id).cloned() else {
        return;
    };
    refresh_view_entries_for(&tab);
}

fn refresh_view_entries_for(tab: &Arc<TabState>) {
    let all_entries = tab.entries.get();
    let nav = tab.navigation.get();
    let current_path = nav.current_path.clone();

    info!(
        "refresh_view_entries: calling filter_entries for path='{}' (len={})",
        current_path,
        all_entries.len()
    );

    let filtered_arch_entries = nav.filter_entries(&all_entries);

    info!(
        "refresh_view_entries: got {} entries for path '{}'",
        filtered_arch_entries.len(),
        current_path
    );

    let entries = filtered_arch_entries
        .iter()
        .map(convert_to_file_entry)
        .collect::<Vec<_>>();

    tab.browser_entries
        .update(|snapshot| snapshot.replace(entries));
}
