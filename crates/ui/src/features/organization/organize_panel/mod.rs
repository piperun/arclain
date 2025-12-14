//! OrganizePanel module
//!
//! Main panel for organizing archive contents with preview.

mod integrity;
mod network_tab;
mod preview_tab;
mod variables_tab;

pub use arclain_core::features::organization::metrics::IntegrityReport;
use integrity::export_issues_report;

use crate::features::organization::export_dialog::ExportTreeDialog;
use crate::shared::dialogs::progress::{ExtractionProgressDialog, ExtractionStatus};

use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::features::organization::{engine::RuleEngine, OrganizationRule};
use arclain_core::features::organization::session::OrganizationSession;
use arclain_core::ArchiveEntry;
use eframe::egui;
use std::sync::mpsc::Receiver;
use std::time::Instant;

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
    pub active_tab: OrganizeTab,
    pub preview_filter: PreviewFilter,
    pub original_tree_state: PreviewTreeState,
    pub organized_tree_state: PreviewTreeState,
    pub original_tree: Vec<preview_tree::PreviewTreeNode>,
    pub organized_tree: Vec<preview_tree::PreviewTreeNode>,
    pub depth_limit: Option<usize>,
    pub export_dialog: ExportTreeDialog,
    pub progress_dialog: ExtractionProgressDialog,
    pub progress_rx: Option<Receiver<ProgressUpdate>>,
    pub organization_child: Option<std::process::Child>,
    pub organization_started: Option<Instant>,
    pub is_organizing: bool,
}

impl Default for OrganizeUiState {
    fn default() -> Self {
        Self {
            selected_rule_index: 0,
            active_tab: OrganizeTab::Preview,
            preview_filter: PreviewFilter::All,
            original_tree_state: PreviewTreeState::default(),
            organized_tree_state: PreviewTreeState::default(),
            original_tree: Vec::new(),
            organized_tree: Vec::new(),
            depth_limit: None,
            export_dialog: ExportTreeDialog::new(),
            progress_dialog: ExtractionProgressDialog::default(),
            progress_rx: None,
            organization_child: None,
            organization_started: None,
            is_organizing: false,
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
        metadata: Option<arclain_core::features::organization::GameMetadata>,
    ) -> Self {
        let session = OrganizationSession::new(
            archive_name,
            entries,
            rules,
            metadata,
        );
        
        let mut panel = Self {
            session,
            ui_state: OrganizeUiState::default(),
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

    /// Update organization progress from channel
    pub fn update_organization_progress(&mut self) {
        if !self.ui_state.is_organizing {
            return;
        }

        // Check for progress updates from channel
        if let Some(ref rx) = self.ui_state.progress_rx {
            while let Ok(update) = rx.try_recv() {
                self.ui_state.progress_dialog.percent = update.percent;
                if let Some(ref msg) = update.message {
                    self.ui_state.progress_dialog.file_action = msg.clone();
                }
                // Check for completion
                if update.percent >= 100 {
                    self.ui_state.progress_dialog.status = ExtractionStatus::Completed;
                    self.ui_state.is_organizing = false;
                    self.ui_state.progress_dialog.show = false;
                }
            }
        }

        // Update elapsed time
        if let Some(started) = self.ui_state.organization_started {
            let elapsed = started.elapsed();
            self.ui_state.progress_dialog.elapsed_text = format!(
                "{}:{:02}",
                elapsed.as_secs() / 60,
                elapsed.as_secs() % 60
            );
        }
    }

    /// Cancel ongoing organization
    pub fn cancel_organization(&mut self) {
        if let Some(ref mut child) = self.ui_state.organization_child {
            let _ = child.kill();
        }
        self.ui_state.organization_child = None;
        self.ui_state.progress_rx = None;
        self.ui_state.is_organizing = false;
        self.ui_state.progress_dialog.show = false;
        self.ui_state.progress_dialog.status = ExtractionStatus::Cancelled;
    }

    fn truncate_path(path: &str, max_len: usize) -> String {
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

    pub fn render(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<OrganizePanelAction> {
        // Update organization progress if running
        self.update_organization_progress();
        
        // Render progress dialog if organizing
        if self.ui_state.progress_dialog.show {
            if let Some(result) = crate::shared::dialogs::progress::render_extraction_progress_dialog(
                ctx,
                &crate::shared::theme::AppTheme::new(false), // TODO: Get actual theme
                &mut self.ui_state.progress_dialog,
            ) {
                match result {
                    crate::shared::dialogs::progress::ExtractionDialogResult::Cancelled => {
                        self.cancel_organization();
                    }
                    _ => {}
                }
            }
        }
        
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

            // Header
            egui::Frame::NONE
                .fill(ui.style().visuals.extreme_bg_color)
                .inner_margin(12.0)
                .corner_radius(8.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::FOLDER_NOTCH_OPEN)
                                .size(28.0)
                                .color(egui::Color32::from_rgb(99, 179, 237)),
                        );
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Organize Archive").size(18.0).strong());
                            ui.label(egui::RichText::new(&self.session.archive_name).size(12.0).weak());
                        });

                        // Metadata badge - smaller with explicit label
                        if let Some(meta) = &self.session.metadata {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // Apply button (enabled/disabled based on metadata)
                                    let apply_btn = egui::Button::new(
                                        egui::RichText::new(format!(
                                            "{}  Apply",
                                            egui_phosphor::regular::CHECK
                                        ))
                                        .strong()
                                        .size(12.0),
                                    )
                                    .fill(if can_apply {
                                        egui::Color32::from_rgb(34, 139, 34)
                                    } else {
                                        egui::Color32::from_rgb(60, 60, 60)
                                    });
                                    
                                    if ui.add_enabled(can_apply, apply_btn).clicked() {
                                        action = Some(OrganizePanelAction::Apply);
                                    }
                                    
                                    ui.add_space(8.0);
                                    
                                    // Metadata badge
                                    egui::Frame::NONE
                                        .fill(egui::Color32::from_rgb(35, 65, 45))
                                        .inner_margin(egui::Margin::symmetric(6, 3))
                                        .corner_radius(3.0)
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} Fetched: {}",
                                                    egui_phosphor::regular::CHECK_CIRCLE,
                                                    Self::truncate_path(&meta.title, 30)
                                                ))
                                                .color(egui::Color32::from_rgb(120, 200, 150))
                                                .size(10.0),
                                            );
                                        });
                                },
                            );
                        } else {
                            // No metadata - show Apply button (possibly disabled)
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let apply_btn = egui::Button::new(
                                        egui::RichText::new(format!(
                                            "{}  Apply",
                                            egui_phosphor::regular::CHECK
                                        ))
                                        .strong()
                                        .size(12.0),
                                    )
                                    .fill(if can_apply {
                                        egui::Color32::from_rgb(34, 139, 34)
                                    } else {
                                        egui::Color32::from_rgb(60, 60, 60)
                                    });
                                    
                                    if ui.add_enabled(can_apply, apply_btn).clicked() {
                                        action = Some(OrganizePanelAction::Apply);
                                    }
                                },
                            );
                        }
                    });
                });

            ui.add_space(4.0);

            // Rule selector
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(egui_phosphor::regular::FUNNEL).size(14.0));
                ui.label(egui::RichText::new("Rule:").strong());

                let current_rule = self
                    .session.rules
                    .get(self.ui_state.selected_rule_index)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "None".to_string());

                let has_dlsite_code = arclain_core::utilities::has_dlsite_code(&self.session.archive_name);

                egui::ComboBox::from_id_salt("rule_selector")
                    .selected_text(&current_rule)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        let mut categories: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
                        for (i, rule) in self.session.rules.iter().enumerate() {
                            let cat = rule.trigger.metadata_source.clone().unwrap_or_else(|| "General".to_string());
                            categories.entry(cat).or_default().push(i);
                        }

                        for (category, indices) in categories {
                            ui.label(
                                egui::RichText::new(category)
                                    .size(10.0)
                                    .strong()
                                    .color(ui.visuals().text_color().gamma_multiply(0.6)),
                            );
                            
                            for i in indices {
                                let rule = &self.session.rules[i];
                                let is_dlsite_rule = rule.trigger.metadata_source.as_deref().map(|s| s.eq_ignore_ascii_case("dlsite")).unwrap_or(false);
                                let is_disabled = is_dlsite_rule && !has_dlsite_code;

                                if is_disabled {
                                    let label = format!("{} (no DLsite code)", rule.name);
                                    ui.add_enabled(
                                        false,
                                        egui::Button::new(egui::RichText::new(label).weak())
                                            .selected(self.ui_state.selected_rule_index == i),
                                    );
                                } else if ui
                                    .selectable_value(&mut self.ui_state.selected_rule_index, i, &rule.name)
                                    .changed()
                                {
                                    self.update_preview();
                                }
                            }
                            ui.add_space(4.0);
                        }
                    });


                // if let Some(rule) = self.session.rules.get(self.ui_state.selected_rule_index) {
                //     if let Some(desc) = &rule.description {
                //         ui.label(egui::RichText::new(desc).weak().italics().size(11.0));
                //     }
                // }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .link(egui::RichText::new("Manage Rules...").size(11.0))
                        .clicked()
                    {
                        action = Some(OrganizePanelAction::ManageRules);
                    }
                });
            });

            ui.separator();

            if missing_metadata {
                self.render_empty_state(ui);
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
                        OrganizeTab::Preview => self.render_preview_tab(ui),
                        OrganizeTab::Variables => self.render_variables_tab(ui),
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

    fn render_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(
                egui::RichText::new(egui_phosphor::regular::X)
                    .size(120.0)
                    .color(egui::Color32::from_rgb(100, 100, 100)),
            );
            ui.add_space(20.0);
            
            ui.label(
                egui::RichText::new("No metadata found")
                    .size(32.0)
                    .strong()
                    .color(egui::Color32::from_rgb(150, 150, 150)),
            );
            ui.add_space(30.0);
            
            ui.label(
                egui::RichText::new("please try and fetch the metadata before trying to organize with dlsite-metadata.")
                    .size(16.0)
                    .color(egui::Color32::from_rgb(120, 120, 120)),
            );
            ui.label(
                egui::RichText::new("If this message still shows up and you have fetched metadata, please check the log.")
                    .size(16.0)
                    .color(egui::Color32::from_rgb(120, 120, 120)),
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
