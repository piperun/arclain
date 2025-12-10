use crate::shared::components::network_log::NetworkLog;
use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
use crate::features::organization::export_dialog::ExportTreeDialog;
use arclain_core::organization::{engine::RuleEngine, OrganizationRule};
use arclain_core::ArchiveEntry;
use eframe::egui::{self, RichText};
use egui_extras::{Size, StripBuilder};
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Default, PartialEq, Clone, Copy)]
pub enum OrganizeTab {
    #[default]
    Preview,
    NetworkActivity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrganizePanelAction {
    Apply,
    LoadScreenshots,
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
    pub screenshot_rx: Option<Receiver<(String, bool)>>,
    pub screenshot_tx: Option<Sender<(String, bool)>>,
    pub is_loading_screenshots: bool,
    // Tree view state
    pub preview_filter: PreviewFilter,
    pub original_tree_state: PreviewTreeState,
    pub organized_tree_state: PreviewTreeState,
    pub original_tree: Vec<preview_tree::PreviewTreeNode>,
    pub organized_tree: Vec<preview_tree::PreviewTreeNode>,
    pub show_variables_legend: bool,
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
            screenshot_rx: None,
            screenshot_tx: None,
            is_loading_screenshots: false,
            preview_filter: PreviewFilter::All,
            original_tree_state: PreviewTreeState::default(),
            organized_tree_state: PreviewTreeState::default(),
            original_tree: Vec::new(),
            organized_tree: Vec::new(),
            show_variables_legend: true,
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
        let (tx, rx) = channel();
        panel.screenshot_tx = Some(tx);
        panel.screenshot_rx = Some(rx);
        
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

        // Check for screenshot updates
        if let Some(rx) = &self.screenshot_rx {
            while let Ok((path, success)) = rx.try_recv() {
                if let Some(plan) = &mut self.preview_plan {
                    for download in &mut plan.downloads {
                        if download.dest_path == path {
                            download.cached = success;
                        }
                    }
                }
                // If all done, we could set is_loading_screenshots = false,
                // but we don't know total count easily here without tracking.
                // For now, let it stay true or rely on something else.
                // Actually, if we just update model, UI reflects it.
            }
        }

        let mut action = None;

        // ════════════════════════════════════════════════════════════════
        // EARLY VALIDATION: Check for DLsite rule without metadata
        // ════════════════════════════════════════════════════════════════
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

                // Check if DLsite code exists in archive name
                let has_dlsite_code = arclain_core::utilities::has_dlsite_code(&self.archive_name);

                egui::ComboBox::from_id_salt("rule_selector")
                    .selected_text(&current_rule)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        // Collect indices by category
                        let mut categories: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
                        for (i, rule) in self.rules.iter().enumerate() {
                            categories.entry(rule.category.clone()).or_default().push(i);
                        }

                        // Render categorized
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
                                    // Gray out DLsite rules when no code detected
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

            // ════════════════════════════════════════════════════════════════
            // EMPTY STATE: Full page takeover when metadata is missing
            // ════════════════════════════════════════════════════════════════
            if missing_metadata {
                // Render full-page empty state (no tabs)
                self.render_empty_state(ui);
            } else {
                // ════════════════════════════════════════════════════════════════
                // TABS: Preview | Network Activity
                // ════════════════════════════════════════════════════════════════
                ui.horizontal(|ui| {
                    // Preview tab
                    let preview_label = format!("{} Preview", egui_phosphor::regular::EYE);
                    let preview_selected = self.active_tab == OrganizeTab::Preview;
                    if ui
                        .selectable_label(
                            preview_selected,
                            egui::RichText::new(&preview_label).size(13.0),
                        )
                        .clicked()
                    {
                        self.active_tab = OrganizeTab::Preview;
                    }

                    ui.add_space(8.0);

                    // Network Activity tab (show count if any)
                    let net_count = self.network_log.len();
                    let net_label = if net_count > 0 {
                        format!(
                            "{} Network Activity ({})",
                            egui_phosphor::regular::GLOBE,
                            net_count
                        )
                    } else {
                        format!("{} Network Activity", egui_phosphor::regular::GLOBE)
                    };
                    let net_selected = self.active_tab == OrganizeTab::NetworkActivity;
                    if ui
                        .selectable_label(net_selected, egui::RichText::new(&net_label).size(13.0))
                        .clicked()
                    {
                        self.active_tab = OrganizeTab::NetworkActivity;
                    }
                });

                ui.add_space(4.0);

                // ════════════════════════════════════════════════════════════════
                // TAB CONTENT
                // ════════════════════════════════════════════════════════════════
                match self.active_tab {
                    OrganizeTab::Preview => self.render_preview_tab(ui, &mut action),
                    OrganizeTab::NetworkActivity => self.render_network_tab(ui),
                }
            }
        });

        // Disable Apply if metadata missing
        if let Some(OrganizePanelAction::Apply) = action {
            let is_dlsite_rule = self
                .rules
                .get(self.selected_rule_index)
                .map(|r| r.category.eq_ignore_ascii_case("dlsite"))
                .unwrap_or(false);
            if is_dlsite_rule && self.metadata.is_none() {
                action = None; // Cancel action
            }
        }

        action
    }

    fn render_empty_state(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            // Large X Icon
            ui.label(
                egui::RichText::new(egui_phosphor::regular::X)
                    .size(120.0)
                    .color(egui::Color32::from_rgb(100, 100, 100)),
            );
            ui.add_space(20.0);
            
            // Heading
            ui.label(
                egui::RichText::new("No metadata found")
                    .size(32.0)
                    .strong()
                    .color(egui::Color32::from_rgb(150, 150, 150)),
            );
            ui.add_space(30.0);
            
            // Subtext
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

    fn render_preview_tab(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<OrganizePanelAction>,
    ) {

        if let Some(plan) = &self.preview_plan.clone() {
            // ════════════════════════════════════════════════════════════════
            // HEADER: Output folder with copy button
            // ════════════════════════════════════════════════════════════════
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(35, 45, 55))
                .inner_margin(10.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(egui_phosphor::regular::FOLDER)
                                .color(egui::Color32::from_rgb(250, 204, 21)),
                        );
                        ui.label(RichText::new("Output:").strong());
                        ui.label(
                            RichText::new(&plan.root_folder)
                                .monospace()
                                .color(egui::Color32::from_rgb(147, 197, 253)),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Copy button
                            if ui
                                .button(RichText::new(format!(
                                    "{} Copy",
                                    egui_phosphor::regular::COPY
                                )))
                                .on_hover_text("Copy folder name to clipboard")
                                .clicked()
                            {
                                ui.ctx().copy_text(plan.root_folder.clone());
                            }

                            ui.add_space(8.0);

                            // Export Tree Button
                            if ui
                                .button(RichText::new(format!(
                                    "{} Export Tree",
                                    egui_phosphor::regular::EXPORT
                                )))
                                .clicked()
                            {
                                self.export_dialog.open();
                            }
                        });
                    });
                });




            ui.add_space(4.0);

            // ════════════════════════════════════════════════════════════════
            // VARIABLES LEGEND (collapsible)
            // ════════════════════════════════════════════════════════════════
            if !plan.resolved_variables.is_empty() {
                let legend_header = format!(
                    "{} Variables {}",
                    egui_phosphor::regular::CODE,
                    if self.show_variables_legend {
                        "▼"
                    } else {
                        "▶"
                    }
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new(&legend_header).size(12.0).weak())
                            .frame(false),
                    )
                    .clicked()
                {
                    self.show_variables_legend = !self.show_variables_legend;
                }

                if self.show_variables_legend {
                    egui::Frame::NONE
                        .fill(ui.style().visuals.faint_bg_color)
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            // Show pattern template
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Pattern:").weak().size(11.0));
                                ui.label(
                                    RichText::new(&plan.root_folder_template)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(250, 204, 21)),
                                );
                                ui.label(
                                    RichText::new("→").weak().size(11.0),
                                );
                                ui.label(
                                    RichText::new(&plan.root_folder)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(134, 239, 172)),
                                );
                            });
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.add_space(4.0);
                            egui::Grid::new("variables_grid")
                                .num_columns(2)
                                .spacing([16.0, 2.0])
                                .show(ui, |ui| {
                                    // Show key variables
                                    for key in ["code", "circle", "title", "version", "product_id"]
                                    {
                                        if let Some(value) = plan.resolved_variables.get(key) {
                                            ui.label(
                                                RichText::new(format!("${}", key))
                                                    .monospace()
                                                    .size(11.0)
                                                    .weak(),
                                            );
                                            ui.label(
                                                RichText::new(Self::truncate_path(value, 40))
                                                    .monospace()
                                                    .size(11.0)
                                                    .color(egui::Color32::from_rgb(147, 197, 253)),
                                            );
                                            ui.end_row();
                                        }
                                    }
                                });
                        });
                }
                ui.add_space(4.0);
            }

            // ════════════════════════════════════════════════════════════════
            // STATS BAR with INTEGRITY VERIFICATION
            // ════════════════════════════════════════════════════════════════
            let report = self.calculate_discrepancies();

            ui.horizontal(|ui| {
                // Original stats
                ui.label(
                    RichText::new(format!(
                        "{} Original: {} files, {} folders",
                        egui_phosphor::regular::ARCHIVE,
                        report.original_files,
                        report.original_folders
                    ))
                    .size(11.0)
                    .weak(),
                );

                ui.separator();

                // Modified stats  
                ui.label(
                    RichText::new(format!(
                        "{} Modified: {} files ({} moved + {} gen + {} dl)",
                        egui_phosphor::regular::FOLDER_NOTCH_OPEN,
                        report.expected_modified_files,
                        report.moved_files,
                        report.generated_files,
                        report.planned_screenshots
                    ))
                    .size(11.0)
                    .weak(),
                );

                // Discrepancy warning
                if report.file_discrepancy != 0 {
                    ui.separator();
                    let discrepancy_text = if report.file_discrepancy > 0 {
                        format!(
                            "{} {} filtered out",
                            egui_phosphor::regular::WARNING,
                            report.file_discrepancy
                        )
                    } else {
                        format!(
                            "{} {} added",
                            egui_phosphor::regular::PLUS,
                            -report.file_discrepancy
                        )
                    };
                    ui.label(
                        RichText::new(discrepancy_text)
                            .size(11.0)
                            .color(if report.file_discrepancy > 0 {
                                egui::Color32::from_rgb(251, 191, 36) // Warning yellow
                            } else {
                                egui::Color32::from_rgb(74, 222, 128) // Success green
                            }),
                    );
                }

                // Screenshot warning
                if report.expected_screenshots != report.planned_screenshots {
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} Screenshots: {}/{} planned",
                            egui_phosphor::regular::IMAGE,
                            report.planned_screenshots,
                            report.expected_screenshots
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                    )
                    .on_hover_text("Some screenshots may not be available or failed to load");
                }
                
                // Fingerprint match indicator
                ui.separator();
                if report.content_match {
                    ui.label(
                        RichText::new(format!("{} Verified", egui_phosphor::regular::CHECK_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(74, 222, 128)), // Green
                    )
                    .on_hover_text(format!(
                        "Content fingerprints match\nOriginal: {:016x}\nContent: {:016x}",
                        report.original_fingerprint, report.content_fingerprint
                    ));
                } else {
                    ui.label(
                        RichText::new(format!("{} Mismatch", egui_phosphor::regular::X_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(248, 113, 113)), // Red
                    )
                    .on_hover_text(format!(
                        "Content fingerprints differ - some files may be missing or extra\nOriginal: {:016x}\nContent: {:016x}",
                        report.original_fingerprint, report.content_fingerprint
                    ));
                }
            });

            ui.horizontal(|ui| {
                if !plan.downloads.is_empty() {
                    if !self.is_loading_screenshots {
                        if ui
                            .button(format!(
                                "{} Load Screenshots",
                                egui_phosphor::regular::DOWNLOAD_SIMPLE
                            ))
                            .on_hover_text("Download screenshots for preview")
                            .clicked()
                        {
                            *action = Some(OrganizePanelAction::LoadScreenshots);
                            self.is_loading_screenshots = true;
                        }
                    } else {
                        ui.spinner();
                        ui.label(RichText::new("Loading...").weak().size(11.0));
                    }
                }

                // Export Issues button - visible when there are discrepancies
                if report.file_discrepancy > 0 || report.expected_screenshots != report.planned_screenshots {
                    ui.separator();
                    if ui
                        .button(format!("{} Export Issues", egui_phosphor::regular::WARNING_CIRCLE))
                        .on_hover_text("Export a report of files filtered out and missing screenshots")
                        .clicked()
                    {
                        Self::export_issues_report(&report, &self.original_tree, &self.organized_tree, &self.metadata);
                    }
                }
            });

            ui.add_space(4.0);

            // ════════════════════════════════════════════════════════════════
            // FILTER TABS & DEPTH LIMIT
            // ════════════════════════════════════════════════════════════════
            ui.horizontal(|ui| {
                let filters = [
                    (PreviewFilter::All, "All"),
                    (PreviewFilter::FoldersOnly, "📁 Folders"),
                    (PreviewFilter::FilesOnly, "📄 Files"),
                    (PreviewFilter::GeneratedOnly, "✨ Generated"),
                ];
                for (filter, label) in filters {
                    if ui
                        .selectable_label(
                            self.preview_filter == filter,
                            RichText::new(label).size(11.0),
                        )
                        .clicked()
                    {
                        self.preview_filter = filter;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("depth_limit")
                        .selected_text(match self.depth_limit {
                            None => "Depth: All".to_string(),
                            Some(0) => "Depth: Root".to_string(),
                            Some(n) => format!("Depth: {}", n),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.depth_limit, None, "All");
                            ui.selectable_value(&mut self.depth_limit, Some(0), "Root Only");
                            ui.selectable_value(&mut self.depth_limit, Some(1), "1 Level");
                            ui.selectable_value(&mut self.depth_limit, Some(2), "2 Levels");
                            ui.selectable_value(&mut self.depth_limit, Some(3), "3 Levels");
                        });
                });
            });

            ui.separator();

            // ════════════════════════════════════════════════════════════════
            // DUAL PANE TREE VIEW
            // ════════════════════════════════════════════════════════════════
            let available = ui.available_size();

            StripBuilder::new(ui)
                .size(Size::remainder().at_least(100.0)) // Left Pane
                .size(Size::exact(30.0)) // Arrow
                .size(Size::remainder().at_least(100.0)) // Right Pane
                .horizontal(|mut strip| {
                    // LEFT PANE: Original structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(30, 30, 35))
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(available.y - 40.0);

                                    let original_title = format!("Original: {}", self.archive_name);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(original_title).strong().size(12.0),
                                        )
                                        .truncate(),
                                    );
                                    ui.separator();

                                    egui::ScrollArea::both()
                                        .id_salt("original_tree")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            preview_tree::render_tree(
                                                ui,
                                                &mut self.original_tree_state,
                                                &self.original_tree,
                                                self.preview_filter,
                                                self.depth_limit,
                                            );
                                        });
                                });
                            });
                    });

                    // ARROW
                    strip.cell(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(available.y / 2.0 - 20.0);
                            ui.label(
                                RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                    .size(20.0)
                                    .color(egui::Color32::from_rgb(74, 222, 128)),
                            );
                        });
                    });

                    // RIGHT PANE: Organized structure
                    strip.cell(|ui| {
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(30, 35, 35))
                            .inner_margin(8.0)
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_height(available.y - 40.0);

                                    let organized_title = format!("Modified: {}", plan.root_folder);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(organized_title).strong().size(12.0),
                                        )
                                        .truncate(),
                                    );
                                    ui.separator();

                                    egui::ScrollArea::both()
                                        .id_salt("organized_tree")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            preview_tree::render_tree(
                                                ui,
                                                &mut self.organized_tree_state,
                                                &self.organized_tree,
                                                self.preview_filter,
                                                self.depth_limit,
                                            );
                                        });
                                });
                            });
                    });
                });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new(egui_phosphor::regular::WARNING)
                        .size(40.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("No preview available").size(14.0).weak());
            });
        }
    }

    fn render_network_tab(&self, ui: &mut egui::Ui) {
        NetworkLog::render(ui, &self.network_log);
    }

    /// Count files in a PreviewTreeNode tree (recursive)
    fn count_files(nodes: &[preview_tree::PreviewTreeNode]) -> usize {
        let mut count = 0;
        for node in nodes {
            if node.is_dir {
                count += Self::count_files(&node.children);
            } else {
                count += 1;
            }
        }
        count
    }

    /// Count folders in a PreviewTreeNode tree (recursive)
    fn count_folders(nodes: &[preview_tree::PreviewTreeNode]) -> usize {
        let mut count = 0;
        for node in nodes {
            if node.is_dir {
                count += 1;
                count += Self::count_folders(&node.children);
            }
        }
        count
    }

    /// Get the number of expected screenshots from metadata
    fn expected_screenshot_count(&self) -> usize {
        self.metadata
            .as_ref()
            .map(|m| m.screenshots.len())
            .unwrap_or(0)
    }

    /// Calculate discrepancies between original and modified trees
    pub fn calculate_discrepancies(&self) -> IntegrityReport {
        let original_file_count = Self::count_files(&self.original_tree);
        let original_folder_count = Self::count_folders(&self.original_tree);

        let expected_screenshots = self.expected_screenshot_count();
        let planned_screenshots = self
            .preview_plan
            .as_ref()
            .map(|p| p.downloads.len())
            .unwrap_or(0);
        let generated_files = self
            .preview_plan
            .as_ref()
            .map(|p| p.generated_files.len())
            .unwrap_or(0);
        let moved_files = self
            .preview_plan
            .as_ref()
            .map(|p| p.moves.len())
            .unwrap_or(0);

        // Files expected in modified = moved + generated + downloads
        let expected_modified_files = moved_files + generated_files + planned_screenshots;

        // Compute fingerprints for quick equality check
        // Compare: original file paths vs source paths from plan.moves
        // This verifies that all original files are accounted for in the organization plan
        
        // 1. Original file paths (strip archive root folder)
        let mut original_set = std::collections::HashSet::new();
        Self::collect_full_paths(&self.original_tree, &mut original_set, "", true);
        let mut original_paths: Vec<String> = original_set.into_iter().collect();
        original_paths.sort();
        
        // 2. Source paths from moves (strip archive root folder - same as original)
        let mut planned_sources: Vec<String> = self
            .preview_plan
            .as_ref()
            .map(|p| {
                p.moves
                    .iter()
                    .map(|(src, _)| {
                        // Strip first path component (archive root)
                        src.split(['/', '\\'])
                            .skip(1)
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        planned_sources.sort();
        
        // FNV-1a hash for fast fingerprinting
        let original_fingerprint = Self::fnv1a_hash(&original_paths.join("\n"));
        let content_fingerprint = Self::fnv1a_hash(&planned_sources.join("\n"));
        let content_match = original_fingerprint == content_fingerprint;

        IntegrityReport {
            original_files: original_file_count,
            original_folders: original_folder_count,
            moved_files,
            generated_files,
            expected_screenshots,
            planned_screenshots,
            expected_modified_files,
            file_discrepancy: original_file_count as i64 - moved_files as i64,
            original_fingerprint,
            content_fingerprint,
            content_match,
        }
    }

    /// FNV-1a hash for fast fingerprinting  
    fn fnv1a_hash(data: &str) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;
        
        let mut hash = FNV_OFFSET;
        for byte in data.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    /// Export a report of all discrepancies (files filtered out, missing screenshots, etc.)
    fn export_issues_report(
        report: &IntegrityReport,
        original_tree: &[preview_tree::PreviewTreeNode],
        organized_tree: &[preview_tree::PreviewTreeNode],
        metadata: &Option<arclain_core::organization::GameMetadata>,
    ) {
        let mut content = String::new();
        content.push_str("=== INTEGRITY REPORT ===\n\n");
        content.push_str(&format!("Generated: {}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));

        // Summary
        content.push_str("## SUMMARY\n\n");
        content.push_str(&format!("Original Files: {}\n", report.original_files));
        content.push_str(&format!("Original Folders: {}\n", report.original_folders));
        content.push_str(&format!("Files Moved: {}\n", report.moved_files));
        content.push_str(&format!("Files Generated: {}\n", report.generated_files));
        content.push_str(&format!("Screenshots Planned: {}\n", report.planned_screenshots));
        content.push_str(&format!("Screenshots Expected: {}\n", report.expected_screenshots));
        content.push_str(&format!("File Discrepancy: {}\n\n", report.file_discrepancy));

        // Files filtered out (in original but not in modified)
        if report.file_discrepancy > 0 {
            content.push_str("## FILES FILTERED OUT\n\n");
            content.push_str("The following files exist in the original archive but will not be included in the organized output:\n\n");

            // Collect all original file full paths
            let mut original_files = std::collections::HashSet::new();
            Self::collect_full_paths(original_tree, &mut original_files, "", false);

            // Collect all modified file destination paths (stripped of root folder prefix)
            let mut modified_files = std::collections::HashSet::new();
            Self::collect_full_paths(organized_tree, &mut modified_files, "", true);

            // Find files in original not in modified
            let mut missing: Vec<_> = original_files
                .difference(&modified_files)
                .cloned()
                .collect();
            missing.sort();

            for file in &missing {
                content.push_str(&format!("  - {}\n", file));
            }

            if missing.is_empty() {
                content.push_str("  (No files filtered out based on tree comparison)\n");
            }
        }

        // Screenshot issues
        if report.expected_screenshots != report.planned_screenshots {
            content.push_str("\n## SCREENSHOT ISSUES\n\n");
            content.push_str(&format!(
                "Expected {} screenshots from metadata, but only {} are planned for download.\n",
                report.expected_screenshots, report.planned_screenshots
            ));
            content.push_str("This may be due to:\n");
            content.push_str("  - Screenshots already cached\n");
            content.push_str("  - Invalid or missing URLs in metadata\n");
            content.push_str("  - Plugin not returning all screenshot URLs\n");
        }

        // Metadata summary
        if let Some(meta) = metadata {
            content.push_str("\n## METADATA SUMMARY\n\n");
            content.push_str(&format!("Title: {}\n", meta.title));
            content.push_str(&format!("Product ID: {}\n", meta.product_id));
            if let Some(creator) = &meta.creator {
                content.push_str(&format!("Creator: {}\n", creator));
            }
            content.push_str(&format!("Screenshots in metadata: {}\n", meta.screenshots.len()));
        }

        // Save the report
        let task = rfd::FileDialog::new()
            .set_file_name("integrity_report.txt")
            .add_filter("Text File", &["txt"])
            .save_file();

        if let Some(path) = task {
            if let Err(e) = std::fs::write(&path, content) {
                tracing::error!("Failed to save integrity report: {}", e);
            } else {
                tracing::info!("Exported integrity report to {:?}", path);
                if let Err(e) = open::that(path) {
                    tracing::warn!("Failed to open exported file: {}", e);
                }
            }
        }
    }

    /// Collect all file full paths from a tree (helper for export_issues_report)
    fn collect_full_paths(
        nodes: &[preview_tree::PreviewTreeNode],
        result: &mut std::collections::HashSet<String>,
        prefix: &str,
        strip_first_component: bool,
    ) {
        for node in nodes {
            let path = if prefix.is_empty() {
                node.name.clone()
            } else {
                format!("{}/{}", prefix, node.name)
            };

            if node.is_dir {
                Self::collect_full_paths(&node.children, result, &path, strip_first_component);
            } else {
                // Optionally strip the first path component for comparison
                let final_path = if strip_first_component {
                    // e.g., "[RJ123]/Game/data/file.json" -> "Game/data/file.json"
                    path.split('/').skip(1).collect::<Vec<_>>().join("/")
                } else {
                    path
                };
                if !final_path.is_empty() {
                    result.insert(final_path);
                }
            }
        }
    }
}

/// Report of integrity statistics
#[derive(Debug, Clone, Default)]
pub struct IntegrityReport {
    pub original_files: usize,
    pub original_folders: usize,
    pub moved_files: usize,
    pub generated_files: usize,
    pub expected_screenshots: usize,
    pub planned_screenshots: usize,
    pub expected_modified_files: usize,
    pub file_discrepancy: i64, // Negative = files filtered out
    /// Fingerprint of original file paths (for quick equality check)
    pub original_fingerprint: u64,
    /// Fingerprint of content file paths in modified tree (excluding generated/downloads)
    pub content_fingerprint: u64,
    /// Whether content fingerprints match (all original content accounted for)
    pub content_match: bool,
}

