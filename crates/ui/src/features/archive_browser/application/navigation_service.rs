//! Navigation application service

use crate::core::signals::AppSignals;

pub struct NavigationService;

impl NavigationService {
    // Navigation workers are the only producers of TabState::browser_entries;
    // renderer-owned sort/filter projections never flow back into signals.
    /// Navigate into a subfolder (RELATIVE to current path)
    pub fn navigate_to_folder(&self, signals: &AppSignals, folder: &str) {
        // Use relative navigation - appends folder to current path
        crate::core::operations::navigation_signals::navigate_to(signals, folder);
        crate::core::operations::navigation_view::refresh_view_entries(signals);
    }

    /// Navigate to an absolute path within the archive (from tree view)
    pub fn navigate_to_path(&self, signals: &AppSignals, path: &str) {
        // Use absolute navigation - replaces current path entirely
        crate::core::operations::navigation_signals::navigate_to_absolute(signals, path);
        crate::core::operations::navigation_view::refresh_view_entries(signals);
    }

    pub fn navigate_back(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_back(signals) {
            crate::core::operations::navigation_view::refresh_view_entries(signals);
        }
    }

    pub fn navigate_forward(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_forward(signals) {
            crate::core::operations::navigation_view::refresh_view_entries(signals);
        }
    }

    pub fn navigate_up(&self, signals: &AppSignals) {
        if crate::core::operations::navigation_signals::navigate_up(signals) {
            crate::core::operations::navigation_view::refresh_view_entries(signals);
        }
    }
}
