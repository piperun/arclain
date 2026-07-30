//! OrganizePanel module
//!
//! Main panel for organizing archive contents with preview.
//!
//! The panel is bound to one open archive session and reads everything
//! it shows through the application facade: the rules and profiles to
//! choose from, the whole-archive file list its "Original" side renders,
//! and the plan preview its "Modified" side renders. It computes
//! nothing about the archive itself -- which is what makes the plan it
//! shows and the plan `Apply` runs the same plan (both name this
//! session, see `arclain_app::operations::OrganizeRequest::
//! archive_session_id`).
//!
//! Render emits intents; the dispatcher
//! (`crate::features::organization::presentation::ui`) runs them, so
//! nothing here awaits the facade mid-frame.

mod header;
mod integrity;
mod network_tab;
mod preview_tab;
mod profile_selector;
mod rule_selector;
mod tab_bar;
mod variables_tab;

use crate::features::organization::export_dialog::ExportTreeDialog;
// ExtractionProgressDialog moved to ArchiveOperations

use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
use crate::shared::theme::AppTheme;
use arclain_app::archive::ProductMetadataSummary;
use arclain_app::ids::ArchiveSessionId;
use arclain_app::organization::{
    OrganizationProfileSummary, OrganizationRuleSummary, OrganizePlanPreview,
};
use eframe::egui;

#[derive(Default, PartialEq, Clone, Copy)]
pub enum OrganizeTab {
    #[default]
    Preview,
    Variables,
    NetworkActivity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrganizePanelAction {
    Apply,
    ManageRules,
    /// The displayed preview no longer belongs to the selected rule (the
    /// user changed rules, metadata arrived, or none has been computed
    /// yet). The dispatcher recomputes it through the facade and hands
    /// the answer back via [`OrganizePanel::set_preview`] /
    /// [`OrganizePanel::set_preview_error`].
    RefreshPreview,
}

pub struct OrganizeUiState {
    pub selected_rule_index: usize,
    pub selected_profile_index: usize,
    pub active_tab: OrganizeTab,
    pub preview_filter: PreviewFilter,
    pub original_tree_state: PreviewTreeState,
    pub organized_tree_state: PreviewTreeState,
    pub original_tree: Vec<preview_tree::PreviewTreeNode>,
    pub organized_tree: Vec<preview_tree::PreviewTreeNode>,
    pub depth_limit: Option<usize>,

    pub export_dialog: ExportTreeDialog,
    // Organization progress handled by ArchiveOperations
}

impl Default for OrganizeUiState {
    fn default() -> Self {
        Self {
            selected_rule_index: 0,
            selected_profile_index: 0,
            active_tab: OrganizeTab::Preview,
            preview_filter: PreviewFilter::All,
            original_tree_state: PreviewTreeState::default(),
            organized_tree_state: PreviewTreeState::default(),
            original_tree: Vec::new(),
            organized_tree: Vec::new(),
            depth_limit: None,

            export_dialog: ExportTreeDialog::new(),
            // Organization now handled by ArchiveOperations
        }
    }
}

pub struct OrganizePanel {
    /// The archive session this panel organizes. Every read it makes and
    /// the organize it applies name this session, so what it previews
    /// and what it applies cannot drift apart.
    pub session_id: ArchiveSessionId,
    pub archive_name: String,
    pub rules: Vec<OrganizationRuleSummary>,
    pub profiles: Vec<OrganizationProfileSummary>,
    /// This session's plugin-reported product metadata, as the facade
    /// summarizes it.
    ///
    /// Nothing about the *organization* cluster is computed from it: the
    /// plan, its integrity report and the rules that apply all come from
    /// the facade, which reads this same metadata off the session
    /// itself. What the panel does with it is display (the fetched-title
    /// badge) and enumerate (the issues export naming the screenshots a
    /// plan did not schedule).
    pub metadata: Option<ProductMetadataSummary>,
    /// The plan for the selected rule, or `None` while one is being
    /// computed for the first time / after a failure.
    preview: Option<OrganizePlanPreview>,
    /// Why the last preview attempt failed. Rendered, and Apply stays
    /// disabled until a preview succeeds -- a stale plan on screen with
    /// a live Apply button underneath it is how the wrong archive gets
    /// organized.
    preview_error: Option<String>,
    /// The rule id the displayed preview (or preview error) was computed
    /// for. `None` re-asks on the next render; the dispatcher stamps it
    /// with whatever it just computed, so a failure is not retried every
    /// frame.
    preview_key: Option<String>,
    /// The archive's own file paths, fetched once per panel: the
    /// "Original" side is the session's content and does not depend on
    /// the selected rule.
    original_paths: Option<Vec<String>>,
    pub network_log: Vec<(std::time::SystemTime, String)>,
    pub ui_state: OrganizeUiState,
}

impl OrganizePanel {
    pub fn new(
        session_id: ArchiveSessionId,
        archive_name: String,
        rules: Vec<OrganizationRuleSummary>,
        profiles: Vec<OrganizationProfileSummary>,
        metadata: Option<ProductMetadataSummary>,
        matching_rule_ids: &[String],
    ) -> Self {
        // Find default profile index
        let default_profile_index = profiles.iter().position(|p| p.is_default).unwrap_or(0);

        // Auto-select: the first enabled rule that actually applies to
        // this archive, else the first enabled one at all.
        let selected_rule_index = rules
            .iter()
            .position(|rule| rule.enabled && matching_rule_ids.contains(&rule.id))
            .or_else(|| rules.iter().position(|rule| rule.enabled))
            .unwrap_or(0);

        let panel = Self {
            session_id,
            archive_name,
            rules,
            profiles,
            metadata,
            preview: None,
            preview_error: None,
            preview_key: None,
            original_paths: None,
            network_log: Vec::new(),
            ui_state: OrganizeUiState {
                selected_rule_index,
                selected_profile_index: default_profile_index,
                ..Default::default()
            },
        };

        tracing::debug!(
            "OrganizePanel::new - {} rules loaded, selected_rule_index={}, selected_rule={}",
            panel.rules.len(),
            panel.ui_state.selected_rule_index,
            panel
                .rules
                .get(panel.ui_state.selected_rule_index)
                .map(|r| format!("'{}'", r.name))
                .unwrap_or("None".to_string())
        );

        panel
    }

    /// The rule whose plan the panel is showing (or about to ask for).
    pub fn selected_rule_id(&self) -> Option<String> {
        self.rules
            .get(self.ui_state.selected_rule_index)
            .map(|rule| rule.id.clone())
    }

    /// The profile the organized output is packed with.
    pub fn selected_profile_id(&self) -> Option<String> {
        self.profiles
            .get(self.ui_state.selected_profile_index)
            .map(|profile| profile.id.clone())
    }

    /// Whether the panel needs the archive's own file list fetched
    /// (once per panel -- it is session content, not rule output).
    pub fn needs_original_paths(&self) -> bool {
        self.original_paths.is_none()
    }

    pub fn set_original_paths(&mut self, paths: Vec<String>) {
        self.ui_state.original_tree = build_original_tree(&paths);
        self.original_paths = Some(paths);
    }

    /// Installs a freshly computed plan for `rule_id`.
    pub fn set_preview(&mut self, preview: OrganizePlanPreview) {
        self.ui_state.organized_tree = build_organized_tree_from(&preview);
        self.preview_key = Some(preview.rule_id.clone());
        self.preview = Some(preview);
        self.preview_error = None;

        if self.metadata.is_some() {
            self.update_network_log(vec![(
                std::time::SystemTime::now(),
                "Metadata applied to preview".to_string(),
            )]);
        }
    }

    /// Records a failed preview attempt for `rule_id`. The previous
    /// plan is dropped rather than left on screen: it describes a rule
    /// the user is no longer looking at.
    pub fn set_preview_error(&mut self, rule_id: String, message: String) {
        self.preview = None;
        self.ui_state.organized_tree = Vec::new();
        self.preview_error = Some(message);
        self.preview_key = Some(rule_id);
    }

    /// Called when this session's plugin metadata changes: the plan is a
    /// function of that metadata, so the next render asks for a fresh
    /// one.
    pub fn metadata_changed(&mut self, metadata: Option<ProductMetadataSummary>) {
        self.metadata = metadata;
        self.preview_key = None;
    }

    pub fn update_network_log(&mut self, log: Vec<(std::time::SystemTime, String)>) {
        self.network_log = log;
    }

    pub fn truncate_path(path: &str, max_len: usize) -> String {
        // Use character count, not byte count, for proper Unicode handling
        let char_count = path.chars().count();
        if char_count <= max_len {
            return path.to_string();
        }
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() <= 2 {
            // Take first half and last half of characters (not bytes)
            let half = max_len / 2;
            let prefix: String = path.chars().take(half).collect();
            let suffix: String = path.chars().skip(char_count - half).collect();
            format!("{}...{}", prefix, suffix)
        } else {
            let first = parts[0];
            let last = parts.last().unwrap();
            format!("{}/.../{}", first, last)
        }
    }

    /// True when the selected rule wants DLsite metadata but the
    /// session has none — used both to gate Apply and to pick between
    /// the empty-state view and the tabbed view.
    fn is_dlsite_rule_without_metadata(&self) -> bool {
        let is_dlsite_rule = self
            .rules
            .get(self.ui_state.selected_rule_index)
            .map(|r| {
                r.trigger
                    .metadata_source
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("dlsite"))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        is_dlsite_rule && self.metadata.is_none()
    }

    /// The plan currently on screen, if any.
    pub fn preview(&self) -> Option<&OrganizePlanPreview> {
        self.preview.as_ref()
    }

    pub fn preview_error(&self) -> Option<&str> {
        self.preview_error.as_deref()
    }

    /// Whether the displayed preview still describes the selected rule.
    fn preview_is_current(&self) -> bool {
        self.preview_key == self.selected_rule_id()
    }

    /// Whether Apply is offered. There must be a plan on screen, it must
    /// be the selected rule's, and the rule must not be waiting on
    /// metadata this session does not have -- applying is running
    /// exactly the plan the panel is showing, so there has to be one.
    pub fn can_apply(&self) -> bool {
        !self.is_dlsite_rule_without_metadata()
            && self.preview.is_some()
            && self.preview_is_current()
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        theme: &AppTheme,
    ) -> Option<OrganizePanelAction> {
        self.ui_state.export_dialog.show(
            ctx,
            &self.ui_state.original_tree,
            &self.ui_state.organized_tree,
            self.metadata.as_ref(),
        );

        let missing_metadata = self.is_dlsite_rule_without_metadata();
        let can_apply = self.can_apply();

        let mut action = None;

        ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

        if let Some(act) = header::render_header(
            ui,
            &self.archive_name,
            self.metadata
                .as_ref()
                .and_then(|meta| meta.title.as_deref()),
            can_apply,
            theme,
        ) {
            action = Some(act);
        }

        ui.add_space(4.0);

        if let Some(act) = rule_selector::render_rule_selector(
            ui,
            &self.archive_name,
            &self.rules,
            &mut self.ui_state.selected_rule_index,
        ) {
            action = Some(act);
        }

        ui.add_space(4.0);

        profile_selector::render_profile_selector(
            ui,
            &self.profiles,
            &mut self.ui_state.selected_profile_index,
        );

        // A failed preview is shown, not swallowed: the panel has no
        // plan to display and Apply stays disabled until one succeeds.
        if let Some(error) = &self.preview_error {
            ui.add_space(4.0);
            ui.colored_label(theme.colors.error, error);
        }

        ui.separator();

        if missing_metadata {
            self.render_empty_state(ui, theme);
        } else {
            if let Some(new_tab) =
                tab_bar::render_tab_bar(ui, self.ui_state.active_tab, self.network_log.len())
            {
                self.ui_state.active_tab = new_tab;
            }

            ui.separator();
            ui.add_space(4.0);

            match self.ui_state.active_tab {
                OrganizeTab::Preview => self.render_preview_tab(ui, theme),
                OrganizeTab::Variables => self.render_variables_tab(ui, theme),
                OrganizeTab::NetworkActivity => self.render_network_tab(ui),
            }
        }

        // Final guard: if a child raised Apply but metadata went missing
        // mid-frame, swallow it so we don't kick off an organize against
        // an incomplete plan.
        if matches!(action, Some(OrganizePanelAction::Apply))
            && self.is_dlsite_rule_without_metadata()
        {
            action = None;
        }

        // A stale (or never-computed) preview outranks the frame's own
        // action only when there is none: a click the user actually made
        // is never dropped in favour of a refresh the next frame will
        // ask for again anyway.
        if action.is_none() && (!self.preview_is_current() || self.needs_original_paths()) {
            action = Some(OrganizePanelAction::RefreshPreview);
        }

        action
    }

    fn render_empty_state(&mut self, ui: &mut egui::Ui, theme: &AppTheme) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(
                egui::RichText::new(egui_phosphor::regular::X)
                    .size(120.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(20.0);

            arclain_widgets::Text::new("No metadata found")
                .size(32.0)
                .strong()
                .color(theme.colors.on_surface_variant)
                .show(ui);
            ui.add_space(30.0);

            ui.label(
                egui::RichText::new("please try and fetch the metadata before trying to organize with dlsite-metadata.")
                    .size(16.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.label(
                egui::RichText::new("If this message still shows up and you have fetched metadata, please check the log.")
                    .size(16.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(40.0);
        });
    }
}

/// The organized side of the preview as a tree. Takes the DTO's own
/// lists, which the facade already sorted, rather than the plan's
/// internal maps the pre-facade panel walked.
fn build_organized_tree_from(preview: &OrganizePlanPreview) -> Vec<preview_tree::PreviewTreeNode> {
    let moves: Vec<(String, String)> = preview
        .moves
        .iter()
        .map(|planned| (planned.source.clone(), planned.destination.clone()))
        .collect();
    let downloads: Vec<String> = preview
        .downloads
        .iter()
        .map(|download| download.destination.clone())
        .collect();
    let variables: Vec<(String, String)> = preview
        .resolved_variables
        .iter()
        .map(|variable| (variable.name.clone(), variable.value.clone()))
        .collect();
    build_organized_tree(&moves, &preview.generated_files, &downloads, &variables)
}
