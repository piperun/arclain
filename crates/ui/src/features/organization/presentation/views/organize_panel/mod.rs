//! OrganizePanel module
//!
//! Main panel for organizing archive contents with preview.

mod integrity;
mod network_tab;
mod preview_tab;
mod variables_tab;
mod header;
mod profile_selector;
mod rule_selector;

pub use arclain_core::features::organization::metrics::IntegrityReport;
use integrity::export_issues_report;

use crate::features::organization::export_dialog::ExportTreeDialog;
// ExtractionProgressDialog moved to ArchiveOperations

use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
use arclain_core::features::organization::{engine::RuleEngine, ArchiveProfile, OrganizationRule};
use arclain_core::features::organization::session::OrganizationSession;
use arclain_core::ArchiveEntry;
use crate::shared::theme::AppTheme;
use eframe::egui;
// std::sync::mpsc::Receiver removed
// std::time::Instant removed

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
}

pub struct OrganizeUiState {
    pub selected_rule_index: usize,
    pub selected_profile_index: usize,
    pub profiles: Vec<ArchiveProfile>,
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
            profiles: Vec::new(),
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
    pub session: OrganizationSession,
    pub ui_state: OrganizeUiState,
}

impl OrganizePanel {
    pub fn new(
        archive_name: String,
        entries: Vec<ArchiveEntry>,
        rules: Vec<OrganizationRule>,
        profiles: Vec<ArchiveProfile>,
        metadata: Option<arclain_core::features::organization::GameMetadata>,
    ) -> Self {
        let session = OrganizationSession::new(
            archive_name,
            entries,
            rules,
            metadata,
        );

        // Find default profile index
        let default_profile_index = profiles
            .iter()
            .position(|p| p.is_default)
            .unwrap_or(0);

        let mut panel = Self {
            session,
            ui_state: OrganizeUiState {
                selected_profile_index: default_profile_index,
                profiles,
                ..Default::default()
            },
        };

        // Auto-select rule
        if let Some(idx) = panel.session.rules.iter().position(|r| {
            r.is_enabled
                && RuleEngine::matches_trigger(
                    &r.trigger,
                    &panel.session.archive_name,
                    &panel.session.entries,
                    panel.session.metadata.as_ref(),
                )
        }) {
            panel.ui_state.selected_rule_index = idx;
        } else if let Some(idx) = panel.session.rules.iter().position(|r| r.is_enabled) {
            panel.ui_state.selected_rule_index = idx;
        }

        panel.update_preview();
        
        // Debug: Log rules and selection
        tracing::debug!(
            "OrganizePanel::new - {} rules loaded, selected_rule_index={}, selected_rule={}",
            panel.session.rules.len(),
            panel.ui_state.selected_rule_index,
            panel.session.rules.get(panel.ui_state.selected_rule_index).map(|r| format!("'{}'", r.name)).unwrap_or("None".to_string())
        );
        
        panel
    }

    pub fn update_network_log(&mut self, log: Vec<(std::time::SystemTime, String)>) {
        self.session.network_log = log;
    }

    pub fn update_preview(&mut self) {
        if let Some(rule) = self.session.rules.get(self.ui_state.selected_rule_index) {
            if let Ok(plan) = RuleEngine::create_plan(
                rule,
                &self.session.archive_name,
                &self.session.entries,
                self.session.metadata.as_ref(),
            ) {
                self.session.preview_plan = Some(plan.clone());

                // Build and cache trees
                // Use self.entries (all archive files) for original tree, NOT plan.moves
                // Filter out directory entries - only include actual files
                let original_paths: Vec<String> = self.session.entries
                    .iter()
                    .filter(|e| !e.is_dir) // Only include files, not directory entries
                    .map(|e| e.path.clone())
                    .collect();

                self.ui_state.original_tree = build_original_tree(&original_paths);

                self.ui_state.organized_tree = build_organized_tree(
                    &plan.moves,
                    &plan.generated_files,
                    &plan.downloads,
                    &plan.resolved_variables,
                );

                if self.session.metadata.is_some() {
                    self.update_network_log(vec![(
                        std::time::SystemTime::now(),
                        "Metadata applied to preview".to_string(),
                    )]);
                }
            }
        }
    }

    /* Organization progress is now handled by ArchiveOperations (global signal) */

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

    #[allow(dead_code)]
    fn filename(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or(path)
    }

    #[allow(dead_code)]
    fn directory(path: &str) -> &str {
        match path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, theme: &AppTheme) -> Option<OrganizePanelAction> {
        
        self.ui_state.export_dialog.show(
            ctx,
            &self.ui_state.original_tree,
            &self.ui_state.organized_tree,
            self.session.metadata.as_ref(),
        );

        let mut action = None;

        // EARLY VALIDATION: Check for DLsite rule without metadata
        let is_dlsite_rule = self
            .session.rules
            .get(self.ui_state.selected_rule_index)
            .map(|r| r.trigger.metadata_source.as_deref().map(|s| s.eq_ignore_ascii_case("dlsite")).unwrap_or(false))
            .unwrap_or(false);
        let missing_metadata = is_dlsite_rule && self.session.metadata.is_none();
        let can_apply = !missing_metadata && self.session.preview_plan.is_some();

        // Main content panel
        {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);

            // Header extracted to header.rs
            if let Some(act) = header::render_header(ui, &self.session, can_apply, theme) {
                action = Some(act);
            }

            ui.add_space(4.0);

            // Rule selector extracted to rule_selector.rs
            let (sel_action, changed) = rule_selector::render_rule_selector(ui, &self.session, &mut self.ui_state.selected_rule_index);
            if let Some(act) = sel_action {
                action = Some(act);
            }
            if changed {
                self.update_preview();
            }

            ui.add_space(4.0);

            // Profile selector
            profile_selector::render_profile_selector(
                ui,
                &self.ui_state.profiles,
                &mut self.ui_state.selected_profile_index,
            );

            ui.separator();

            if missing_metadata {
                self.render_empty_state(ui, theme);
            } else {
                    ui.horizontal(|ui| {
                        // Tab Selector
                        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                        
                        let tab_btn = |ui: &mut egui::Ui, label: &str, tab: OrganizeTab, active: OrganizeTab| {
                            let is_active = tab == active;
                            let text = egui::RichText::new(label)
                                .size(13.0)
                                .color(if is_active {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().text_color().gamma_multiply(0.6)
                                });
                                
                            if ui
                                .add(egui::Button::new(text).frame(false))
                                .clicked()
                            {
                                return Some(tab);
                            }
                            None
                        };

                        if let Some(tab) = tab_btn(ui, &format!("{} Preview", egui_phosphor::regular::EYE), OrganizeTab::Preview, self.ui_state.active_tab) {
                            self.ui_state.active_tab = tab;
                        }
                        
                        ui.add_space(16.0);
                        
                        if let Some(tab) = tab_btn(ui, &format!("{} Variables", egui_phosphor::regular::CODE), OrganizeTab::Variables, self.ui_state.active_tab) {
                            self.ui_state.active_tab = tab;
                        }

                        ui.add_space(16.0);

                        let net_count = self.session.network_log.len();
                        let net_label = if net_count > 0 {
                            format!("{} Network ({})", egui_phosphor::regular::GLOBE, net_count)
                        } else {
                            format!("{} Network", egui_phosphor::regular::GLOBE)
                        };

                        if let Some(tab) = tab_btn(ui, &net_label, OrganizeTab::NetworkActivity, self.ui_state.active_tab) {
                            self.ui_state.active_tab = tab;
                        }
                    });

                    ui.separator();
                    ui.add_space(4.0);

                    match self.ui_state.active_tab {
                        OrganizeTab::Preview => self.render_preview_tab(ui, theme),
                        OrganizeTab::Variables => self.render_variables_tab(ui, theme),
                        OrganizeTab::NetworkActivity => self.render_network_tab(ui),
                    }
                }
        }

        if let Some(OrganizePanelAction::Apply) = action {
            let is_dlsite_rule = self
                .session.rules
                .get(self.ui_state.selected_rule_index)
                .map(|r| r.trigger.metadata_source.as_deref().map(|s| s.eq_ignore_ascii_case("dlsite")).unwrap_or(false))
                .unwrap_or(false);
            if is_dlsite_rule && self.session.metadata.is_none() {
                action = None;
            }
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



    pub fn export_issues_report(
        report: &IntegrityReport,
        original_tree: &[preview_tree::PreviewTreeNode],
        organized_tree: &[preview_tree::PreviewTreeNode],
        metadata: &Option<arclain_core::features::organization::GameMetadata>,
    ) {
        export_issues_report(report, original_tree, organized_tree, metadata);
    }
}
