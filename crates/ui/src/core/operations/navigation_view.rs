//! Navigation view utilities
//!
//! Helper functions for updating view state after navigation changes.

use crate::core::signals::AppSignals;
use crate::core::utils::convert_to_file_entry;
use tracing::info;

/// Re-filter view_entries based on the current navigation path.
/// Call this after updating navigation.current_path to sync the file list.
pub fn refresh_view_entries(signals: &AppSignals) {
    let tab = signals.tabs.get().active().clone();
    let all_entries = tab.entries.get();
    let nav = tab.navigation.get();
    let current_path = nav.current_path.clone();

    info!(
        "refresh_view_entries: calling filter_entries for path='{}' (len={})",
        current_path,
        all_entries.len()
    );

    // Use the NavigationState's filter logic to ensure consistency with tree view
    // This handles path normalization, folder rollup, and size calculation correctly.
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

    tab.browser_view_state.update(|s| {
        s.view_entries = entries;
    });
}
