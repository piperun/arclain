//! OrganizePanel module
//!
//! Main panel for organizing archive contents with preview.

mod integrity;
mod network_tab;
mod preview_tab;
mod variables_tab;

pub use integrity::IntegrityReport;
use integrity::{collect_full_paths, count_files, count_folders, export_issues_report, fnv1a_hash};

use crate::features::organization::export_dialog::ExportTreeDialog;

use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
use arclain_core::organization::{engine::RuleEngine, OrganizationRule};
use arclain_core::ArchiveEntry;
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
}

pub struct OrganizePanel {
    pub archive_name: String,
    pub entries: Vec<ArchiveEntry>,
    pub rules: Vec<OrganizationRule>,
    pub selected_rule_index: usize,
    pub preview_plan: Option<arclain_core::organization::engine::OrganizationPlan>,
    pub metadata: Option<arclain_core::organization::GameMetadata>,
    pub network_log: Vec<(std::time::SystemTime, String)>,
    pub active_tab: OrganizeTab,
    // Tree view state
    pub preview_filter: PreviewFilter,
    pub original_tree_state: PreviewTreeState,
    pub organized_tree_state: PreviewTreeState,
    pub original_tree: Vec<preview_tree::PreviewTreeNode>,
    pub organized_tree: Vec<preview_tree::PreviewTreeNode>,
    pub depth_limit: Option<usize>,
    pub export_dialog: ExportTreeDialog,
}

impl OrganizePanel {
    pub fn new(
        archive_name: String,
        entries: Vec<ArchiveEntry>,
        rules: Vec<OrganizationRule>,
        metadata: Option<arclain_core::organization::GameMetadata>,
    ) -> Self {
        let mut panel = Self {
            archive_name: archive_name.clone(),
            entries: entries.clone(),
            rules: rules.clone(),
            selected_rule_index: 0,
            preview_plan: None,
            metadata,
            network_log: Vec::new(),
            active_tab: OrganizeTab::Preview,
            preview_filter: PreviewFilter::All,
            original_tree_state: PreviewTreeState::default(),
            organized_tree_state: PreviewTreeState::default(),
            original_tree: Vec::new(),
            organized_tree: Vec::new(),
            depth_limit: None,
            export_dialog: ExportTreeDialog::new(),
        };

        // Auto-select rule
        if let Some(idx) = panel.rules.iter().position(|r| {
            r.is_enabled
                && RuleEngine::matches_trigger(
                    &r.trigger,
                    &panel.archive_name,
                    &panel.entries,
                    panel.metadata.as_ref(),
                )
        }) {
            panel.selected_rule_index = idx;
        } else if let Some(idx) = panel.rules.iter().position(|r| r.is_enabled) {
            panel.selected_rule_index = idx;
        }

        panel.update_preview();
        
        // Debug: Log rules and selection
        tracing::debug!(
            "OrganizePanel::new - {} rules loaded, selected_rule_index={}, selected_rule={}",
            panel.rules.len(),
            panel.selected_rule_index,
            panel.rules.get(panel.selected_rule_index).map(|r| format!("'{}' (category: '{}')", r.name, r.category)).unwrap_or("None".to_string())
        );
        
        panel
    }

    pub fn update_network_log(&mut self, log: Vec<(std::time::SystemTime, String)>) {
        self.network_log = log;
    }

    pub fn update_preview(&mut self) {
        if let Some(rule) = self.rules.get(self.selected_rule_index) {
            if let Ok(plan) = RuleEngine::create_plan(
                rule,
                &self.archive_name,
                &self.entries,
                self.metadata.as_ref(),
            ) {
                self.preview_plan = Some(plan.clone());

                // Build and cache trees
                // Use self.entries (all archive files) for original tree, NOT plan.moves
                // Filter out directory entries - only include actual files
                let original_paths: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| !e.is_dir) // Only include files, not directory entries
                    .map(|e| e.path.clone())
                    .collect();

                self.original_tree = build_original_tree(&original_paths);

                self.organized_tree = build_organized_tree(
                    &plan.moves,
                    &plan.generated_files,
                    &plan.downloads,
                    &plan.resolved_variables,
                );

                if self.metadata.is_some() {
                    self.update_network_log(vec![(
                        std::time::SystemTime::now(),
                        "Metadata applied to preview".to_string(),
                    )]);
                }
            }
        }
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

    pub fn render(&mut self, ctx: &egui::Context) -> Option<OrganizePanelAction> {
        self.export_dialog.show(
            ctx,
            &self.original_tree,
            &self.organized_tree,
            self.metadata.as_ref(),
        );

        let mut action = None;

        // EARLY VALIDATION: Check for DLsite rule without metadata
        let is_dlsite_rule = self
            .rules
            .get(self.selected_rule_index)
            .map(|r| r.category.eq_ignore_ascii_case("dlsite"))
            .unwrap_or(false);
        let missing_metadata = is_dlsite_rule && self.metadata.is_none();
        let can_apply = !missing_metadata && self.preview_plan.is_some();

        // Main content panel
        egui::CentralPanel::default().show(ctx, |ui| {
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
                            ui.label(egui::RichText::new(&self.archive_name).size(12.0).weak());
                        });

                        // Metadata badge - smaller with explicit label
                        if let Some(meta) = &self.metadata {
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
                    .rules
                    .get(self.selected_rule_index)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "None".to_string());

                let has_dlsite_code = arclain_core::utilities::has_dlsite_code(&self.archive_name);

                egui::ComboBox::from_id_salt("rule_selector")
                    .selected_text(&current_rule)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        let mut categories: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
                        for (i, rule) in self.rules.iter().enumerate() {
                            categories.entry(rule.category.clone()).or_default().push(i);
                        }

                        for (category, indices) in categories {
                            ui.label(
                                egui::RichText::new(category)
                                    .size(10.0)
                                    .strong()
                                    .color(ui.visuals().text_color().gamma_multiply(0.6)),
                            );
                            
                            for i in indices {
                                let rule = &self.rules[i];
                                let is_dlsite_rule = rule.category.to_lowercase() == "dlsite";
                                let is_disabled = is_dlsite_rule && !has_dlsite_code;

                                if is_disabled {
                                    let label = format!("{} (no DLsite code)", rule.name);
                                    ui.add_enabled(
                                        false,
                                        egui::Button::new(egui::RichText::new(label).weak())
                                            .selected(self.selected_rule_index == i),
                                    );
                                } else if ui
                                    .selectable_value(&mut self.selected_rule_index, i, &rule.name)
                                    .changed()
                                {
                                    self.update_preview();
                                }
                            }
                            ui.add_space(4.0);
                        }
                    });


                if let Some(rule) = self.rules.get(self.selected_rule_index) {
                    if let Some(desc) = &rule.description {
                        ui.label(egui::RichText::new(desc).weak().italics().size(11.0));
                    }
                }

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

                        if let Some(tab) = tab_btn(ui, &format!("{} Preview", egui_phosphor::regular::EYE), OrganizeTab::Preview, self.active_tab) {
                            self.active_tab = tab;
                        }
                        
                        ui.add_space(16.0);
                        
                        if let Some(tab) = tab_btn(ui, &format!("{} Variables", egui_phosphor::regular::CODE), OrganizeTab::Variables, self.active_tab) {
                            self.active_tab = tab;
                        }

                        ui.add_space(16.0);

                        let net_count = self.network_log.len();
                        let net_label = if net_count > 0 {
                            format!("{} Network ({})", egui_phosphor::regular::GLOBE, net_count)
                        } else {
                            format!("{} Network", egui_phosphor::regular::GLOBE)
                        };

                        if let Some(tab) = tab_btn(ui, &net_label, OrganizeTab::NetworkActivity, self.active_tab) {
                            self.active_tab = tab;
                        }
                    });

                    ui.separator();
                    ui.add_space(4.0);

                    match self.active_tab {
                        OrganizeTab::Preview => self.render_preview_tab(ui),
                        OrganizeTab::Variables => self.render_variables_tab(ui),
                        OrganizeTab::NetworkActivity => self.render_network_tab(ui),
                    }
                }
            });

        if let Some(OrganizePanelAction::Apply) = action {
            let is_dlsite_rule = self
                .rules
                .get(self.selected_rule_index)
                .map(|r| r.category.eq_ignore_ascii_case("dlsite"))
                .unwrap_or(false);
            if is_dlsite_rule && self.metadata.is_none() {
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

    /// Get the number of expected screenshots from metadata
    fn expected_screenshot_count(&self) -> usize {
        self.metadata
            .as_ref()
            .map(|m| m.screenshots.len())
            .unwrap_or(0)
    }

    /// Calculate discrepancies using source coverage
    pub fn calculate_discrepancies(&self) -> IntegrityReport {
        let original_file_count = count_files(&self.original_tree);
        let original_folder_count = count_folders(&self.original_tree);

        let expected_screenshots = self.expected_screenshot_count();
        let planned_screenshots = self
            .preview_plan
            .as_ref()
            .map(|p| p.downloads.len())
            .unwrap_or(0);
        let generated_files_count = self
            .preview_plan
            .as_ref()
            .map(|p| p.generated_files.len())
            .unwrap_or(0);
        let moved_files = self
            .preview_plan
            .as_ref()
            .map(|p| p.moves.len())
            .unwrap_or(0);

        let expected_modified_files = moved_files + generated_files_count + planned_screenshots;
        
        
        // 1. Original file paths
        let mut original_set = std::collections::HashSet::new();
        collect_full_paths(&self.original_tree, &mut original_set, "");
        let mut original_paths: Vec<String> = original_set.iter().cloned().collect();
        original_paths.sort();
        let original_hash = fnv1a_hash(&original_paths.join("|"));
        
        // 2. Covered paths from plan (Source Hashing)
        // We hash the SOURCE paths of the moves plan to verify that the set of files 
        // being organized is identical to the original set of files.
        // CRITICAL: Normalize path separators to '/' because preview_tree normalizes original paths to '/'.
        // If plan.moves contains '\' (on Windows), straight comparison fails.
        let mut plan_sources: Vec<String> = if let Some(plan) = &self.preview_plan {
             plan.moves.iter().map(|(src, _)| src.replace('\\', "/")).collect()
        } else {
             Vec::new()
        };
        // Also add logic to verify if we missed any original files
        let covered_set: std::collections::HashSet<String> = plan_sources.iter().cloned().collect();
        let missing_original_files: Vec<String> = original_set.difference(&covered_set).cloned().collect();
        
        plan_sources.sort();
        let result_hash = fnv1a_hash(&plan_sources.join("|"));

        let content_match = original_hash == result_hash;

        let modified_file_count = count_files(&self.organized_tree);
        // This discrepancy calc is still vague but let's keep it for "total count"
        let expected_total = original_file_count + generated_files_count + planned_screenshots;
        let file_discrepancy = (modified_file_count as i64) - (expected_total as i64);

        IntegrityReport {
            original_files: original_file_count,
            original_folders: original_folder_count,
            moved_files,
            generated_files: generated_files_count,
            expected_screenshots,
            planned_screenshots,
            expected_modified_files,
            file_discrepancy,
            missing_original_files,
            original_hash,
            result_hash,
            content_match,
        }
    }

    pub fn export_issues_report(
        report: &IntegrityReport,
        original_tree: &[preview_tree::PreviewTreeNode],
        organized_tree: &[preview_tree::PreviewTreeNode],
        metadata: &Option<arclain_core::organization::GameMetadata>,
    ) {
        export_issues_report(report, original_tree, organized_tree, metadata);
    }
}
