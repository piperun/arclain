//! Navigation application service

use crate::core::signals::AppSignals;
use crate::core::utils::convert_to_file_entry;
use std::path::PathBuf;

pub struct NavigationService;

impl NavigationService {
    pub fn navigate_to_folder(&self, signals: &AppSignals, folder: &str) {
        crate::core::operations::navigation_signals::navigate_to_absolute(signals, folder);
        self.update_local_state(signals);
    }

    pub fn navigate_to_path(&self, signals: &AppSignals, path: &str) {
        crate::core::operations::navigation_signals::navigate_to_absolute(signals, path);
        self.update_local_state(signals);
    }

    pub fn navigate_back(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_back(signals) {
            self.update_local_state(signals);
        }
    }

    pub fn navigate_forward(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_forward(signals) {
            self.update_local_state(signals);
        }
    }

    pub fn navigate_up(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_up(signals) {
            self.update_local_state(signals);
        }
    }

    /// Helper to update local state from signals (lock-free)
    fn update_local_state(&self, signals: &AppSignals) {
        // Re-filter entries based on new navigation path
        let all_entries = signals.entries.get();
        let current_path = signals.navigation.get().current_path;
        let current_archive = signals.archive_path.get();

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

        let entries = current_entries
            .iter()
            .map(convert_to_file_entry)
            .collect::<Vec<_>>();
        let display_path = self.update_current_path_display(current_path, current_archive);

        signals.browser_view_state.update(|s| {
            s.view_entries = entries;
            s.current_path = display_path;
        });
    }

    fn update_current_path_display(&self, path: String, archive: Option<PathBuf>) -> String {
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
}
