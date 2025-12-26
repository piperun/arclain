use super::ArchiveBrowserState;
use crate::core::utils::convert_to_file_entry;
use crate::shared::SharedState;
use std::path::PathBuf;

pub fn navigate_to_folder(state: &mut ArchiveBrowserState, shared: &SharedState, folder: &str) {
    let app_state = shared.app_state.lock();
    crate::core::operations::navigation_signals::navigate_to(&app_state.signals, folder);

    // Update local state from signals
    update_local_state(state, &app_state.signals, &app_state.current_archive);
}

pub fn navigate_to_path(state: &mut ArchiveBrowserState, shared: &SharedState, path: &str) {
    let app_state = shared.app_state.lock();
    crate::core::operations::navigation_signals::navigate_to_absolute(&app_state.signals, path);

    // Update local state from signals
    update_local_state(state, &app_state.signals, &app_state.current_archive);
}

pub fn navigate_back(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let app_state = shared.app_state.lock();
    if crate::core::operations::navigation_signals::navigate_back(&app_state.signals) {
        update_local_state(state, &app_state.signals, &app_state.current_archive);
    }
}

pub fn navigate_forward(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let app_state = shared.app_state.lock();
    if crate::core::operations::navigation_signals::navigate_forward(&app_state.signals) {
        update_local_state(state, &app_state.signals, &app_state.current_archive);
    }
}

pub fn navigate_up(state: &mut ArchiveBrowserState, shared: &SharedState) {
    let app_state = shared.app_state.lock();
    if crate::core::operations::navigation_signals::navigate_up(&app_state.signals) {
        update_local_state(state, &app_state.signals, &app_state.current_archive);
    }
}

// Helper to update local state from signals
fn update_local_state(
    state: &mut ArchiveBrowserState,
    signals: &crate::core::signals::AppSignals,
    current_archive: &Option<std::path::PathBuf>,
) {
    // Re-filter entries based on new navigation path
    let all_entries = signals.entries.get();
    let current_path = signals.navigation.get().current_path;

    // Filter logic duplicated from AppState::get_current_entries for now
    let current_entries: Vec<_> = all_entries
        .iter()
        .filter(|e| {
            let parent = std::path::Path::new(&e.path)
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();

            if current_path.is_empty() {
                // Return entries with no parent or empty parent, but not empty path itself
                !e.path.is_empty() && (parent.is_empty() || parent == ".")
            } else {
                parent == current_path
            }
        })
        .cloned()
        .collect();

    state.entries = current_entries.iter().map(convert_to_file_entry).collect();

    state.current_path = update_current_path_display(current_path, current_archive.clone());
}

fn update_current_path_display(path: String, archive: Option<PathBuf>) -> String {
    if let Some(archive_path) = archive {
        if let Some(archive_name) = archive_path.file_name() {
            if let Some(name) = archive_name.to_str() {
                if path.is_empty() {
                    return format!("{}/", name);
                } else {
                    return format!("{}/{}", name, path);
                }
            }
        }
    }
    path
}
