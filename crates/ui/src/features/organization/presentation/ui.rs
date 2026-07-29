use crate::features::organization::application::facade;
use crate::features::organization::presentation::views::{ProfilesPage, RulesPage};
use crate::features::organization::OrganizerPage;

use crate::shared::SharedState;
use eframe::egui;

// Re-export OrganizationAction from domain
pub use crate::features::organization::domain::types::OrganizationAction;

pub struct OrganizationFeature {
    pub organizer_page: Option<OrganizerPage>,
    pub rules_page: RulesPage,
    pub profiles_page: ProfilesPage,
}

impl OrganizationFeature {
    pub fn new(_shared: &SharedState) -> Self {
        Self {
            organizer_page: None,
            rules_page: RulesPage::new(),
            profiles_page: ProfilesPage::new(),
        }
    }

    // ensure_rules_loaded removed as RulesPage handles it internally

    pub fn render(&mut self, ctx: &egui::Context, shared: &SharedState) -> OrganizationAction {
        let mut action = OrganizationAction::None;

        if let Some(page) = &mut self.organizer_page {
            if let Some(result) = page.render(ctx, &shared.theme) {
                match result {
                    crate::features::organization::OrganizePanelAction::Apply => {
                        action = OrganizationAction::Apply;
                    }
                    crate::features::organization::OrganizePanelAction::ManageRules => {
                        action = OrganizationAction::ManageRules;
                    }
                    crate::features::organization::OrganizePanelAction::RefreshPreview => {
                        refresh_preview(&mut page.panel, shared);
                    }
                }
            }
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("No active organization session.");
            });
        }

        action
    }
}

/// Recomputes what the panel shows for its currently selected rule: the
/// plan preview, plus (once per panel) the archive's own file list.
///
/// The panel emits this intent whenever the displayed plan stops
/// belonging to the selected rule — a rule change, or metadata arriving
/// for the session. A failure is recorded against the rule that
/// produced it, so the panel renders the reason instead of leaving the
/// previous rule's plan on screen with Apply still live.
pub fn refresh_preview(
    panel: &mut crate::features::organization::OrganizePanel,
    shared: &SharedState,
) {
    let Some((app, runtime)) = facade::handles(shared) else {
        return;
    };

    if panel.needs_original_paths() {
        // Recorded even when it fails (as an empty list): the panel asks
        // for this exactly as long as it has none, and a session that
        // cannot answer will not answer on the next frame either --
        // retrying forever would mean one facade call per rendered
        // frame.
        let paths = runtime
            .block_on(app.archive_file_paths(panel.session_id))
            .unwrap_or_else(|error| {
                tracing::warn!(
                    "organize panel: could not read the archive's file list: {}",
                    error.summary
                );
                Vec::new()
            });
        panel.set_original_paths(paths);
    }

    let Some(rule_id) = panel.selected_rule_id() else {
        return;
    };
    match runtime.block_on(app.preview_organize_plan(panel.session_id, rule_id.clone())) {
        Ok(preview) => panel.set_preview(preview),
        Err(error) => panel.set_preview_error(
            rule_id,
            facade::describe("This rule produced no plan", &error),
        ),
    }
}
