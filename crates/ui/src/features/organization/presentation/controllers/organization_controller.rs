//! Organization feature actions
//!
//! This module defines the actions that can be triggered by the organization UI
//! and provides the handler context for processing them.

use crate::core::navigation::PageNavigator;
use crate::features::organization::OrganizationFeature;
use crate::shared::components::StatusBarInfo;
use crate::shared::SharedState;

// Re-export action types from existing modules
pub use crate::features::organization::OrganizationAction;

/// Context required for handling organization feature actions
pub struct ActionContext<'a> {
    pub shared: &'a SharedState,
    pub organization_feature: &'a mut OrganizationFeature,
    pub page_navigator: &'a mut PageNavigator,
    pub status_info: &'a mut StatusBarInfo,
}

impl<'a> ActionContext<'a> {
    /// Handle organization actions
    pub fn handle(&mut self, action: &OrganizationAction) -> bool {
        match action {
            OrganizationAction::Apply => {
                // Apply organization plan
                if let Some(page) = &self.organization_feature.organizer_page {
                    if let Some(plan) = &page.panel.session.preview_plan {
                        // We need to clone shared state for the async operation
                        let shared_state = self.shared.clone();

                        let archive_path = self.shared.signals().tabs.get().active().archive_path.get();

                        if let Some(path) = archive_path {
                            // Get selected profile from organizer page UI state
                            let profile = page
                                .panel
                                .ui_state
                                .profiles
                                .get(page.panel.ui_state.selected_profile_index)
                                .cloned();

                            // Build destination path by changing extension based on profile
                            let dest_ext = profile.as_ref().map(|p| p.format.extension()).unwrap_or("7z");
                            let dest_path = path.with_extension(dest_ext);

                            // Run asynchronously via ArchiveOperations
                            crate::features::archive_operations::run_organization_plan(
                                shared_state,
                                plan.clone(),
                                path,
                                dest_path,
                                profile,
                            );
                            self.status_info.message = "Organization started...".to_string();
                        }
                    }
                }

                // Close organizer page and navigate back
                self.organization_feature.organizer_page = None;
                self.page_navigator.navigate_back();
                true
            }
            OrganizationAction::ManageRules => {
                // Navigate to rules settings
                self.page_navigator
                    .navigate_to(crate::core::AppPage::Settings(
                        crate::core::SettingsPage::OrganizationRules,
                    ));
                true
            }
            OrganizationAction::None => false,
        }
    }
}
