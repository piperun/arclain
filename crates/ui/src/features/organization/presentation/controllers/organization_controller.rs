//! Organization feature actions
//!
//! This module defines the actions that can be triggered by the organization UI
//! and provides the handler context for processing them.

use crate::core::navigation::PageNavigator;
use crate::features::organization::application::facade;
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
                if let Some(page) = &self.organization_feature.organizer_page {
                    self.status_info.message = start_organize(self.shared, &page.panel);
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

/// Runs the plan the panel is showing, as a registered, cancellable,
/// event-streamed operation.
///
/// The request names the panel's own archive session rather than a
/// path, which is what makes this *the previewed plan*: the application
/// then organizes that session's archive from that session's metadata,
/// the same two things the preview on screen was computed from. Nothing
/// here rebuilds a plan of its own.
///
/// Returns the status-bar message describing what happened.
fn start_organize(
    shared: &SharedState,
    panel: &crate::features::organization::OrganizePanel,
) -> String {
    let Some((app, runtime)) = facade::handles(shared) else {
        return facade::unavailable();
    };
    let (Some(rule_id), Some(profile_id)) = (panel.selected_rule_id(), panel.selected_profile_id())
    else {
        return "Organization needs both a rule and a profile.".to_string();
    };

    // The organized archive is written beside the one it was built from,
    // as the pre-facade quick action did. Its *name* is the
    // application's own convention (the resolved metadata title, else a
    // detected product code, else the source stem) rather than the
    // source file's name with a new extension.
    let tab = shared.signals().tabs.get().active().clone();
    let Some(destination) = tab
        .archive_path
        .get()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    else {
        return "Organization needs an open archive.".to_string();
    };

    let request = arclain_app::operations::organize::OrganizeRequest {
        // Empty by contract: the session names the archive, so this
        // cannot organize anything other than what was previewed.
        inputs: Vec::new(),
        destination,
        profile_id,
        rule_id,
        dry_run: false,
        archive_session_id: Some(panel.session_id),
    };

    match runtime.block_on(app.start_organize(request)) {
        Ok(operation_id) => {
            // Registering the origin tab is what routes this operation's
            // own password challenge to that tab's dialog, and its
            // progress to the status bar (see `core::operation_bridge`).
            runtime.block_on(crate::core::operation_bridge::register_operation(
                shared,
                operation_id,
                tab.id,
            ));
            "Organization started...".to_string()
        }
        Err(error) => facade::describe("Organization did not start", &error),
    }
}
