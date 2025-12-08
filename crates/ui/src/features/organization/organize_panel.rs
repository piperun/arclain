use crate::shared::components::network_log::NetworkLog;
use crate::shared::components::preview_tree::{
    self, build_organized_tree, build_original_tree, PreviewFilter, PreviewTreeState,
};
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
    Cancel,
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
    // Tree view state
    pub preview_filter: PreviewFilter,
    pub original_tree_state: PreviewTreeState,
    pub organized_tree_state: PreviewTreeState,
    pub original_tree: Vec<preview_tree::PreviewTreeNode>,
    pub organized_tree: Vec<preview_tree::PreviewTreeNode>,
    pub show_variables_legend: bool,
    pub depth_limit: Option<usize>,
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
        };

        // Auto-select rule
        if let Some(meta) = &panel.metadata {
            if let Some(idx) = panel
                .rules
                .iter()
                .position(|r| r.is_enabled && r.category.eq_ignore_ascii_case(&meta.source))
            {
                panel.selected_rule_index = idx;
            } else if let Some(idx) = panel.rules.iter().position(|r| r.is_enabled) {
                panel.selected_rule_index = idx;
            }
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
                let original_paths: Vec<String> =
                    plan.moves.iter().map(|(src, _)| src.clone()).collect();
                self.original_tree = build_original_tree(&original_paths);

                println!("DEBUG: Plan moves count: {}", plan.moves.len());
                if let Some(first) = plan.moves.first() {
                    println!("DEBUG: First move: src='{}', dst='{}'", first.0, first.1);
                }

                self.organized_tree = build_organized_tree(
                    &plan.moves,
                    &plan.generated_files,
                    &plan.downloads,
                    &plan.resolved_variables,
                );
                println!("DEBUG: Organized tree nodes: {}", self.organized_tree.len());

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
        if path.len() <= max_len {
            return path.to_string();
        }
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() <= 2 {
            let half = max_len / 2;
            format!("{}...{}", &path[..half], &path[path.len() - half..])
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

        // Bottom action bar
        egui::TopBottomPanel::bottom("organize_actions")
            .frame(
                egui::Frame::NONE
                    .fill(ctx.style().visuals.window_fill)
                    .inner_margin(12.0),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  Cancel", egui_phosphor::regular::X))
                        .clicked()
                    {
                        action = Some(OrganizePanelAction::Cancel);
                    }
                    ui.add_space(12.0);
                    let apply_btn = egui::Button::new(
                        egui::RichText::new(format!(
                            "{}  Apply Organization",
                            egui_phosphor::regular::CHECK
                        ))
                        .strong(),
                    )
                    .fill(egui::Color32::from_rgb(34, 139, 34));
                    if ui.add(apply_btn).clicked() {
                        action = Some(OrganizePanelAction::Apply);
                    }
                });
            });

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

                        // Metadata badge
                        if let Some(meta) = &self.metadata {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    egui::Frame::NONE
                                        .fill(egui::Color32::from_rgb(45, 85, 55))
                                        .inner_margin(egui::Margin::symmetric(8, 4))
                                        .corner_radius(4.0)
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} {}",
                                                    egui_phosphor::regular::CHECK_CIRCLE,
                                                    &meta.title
                                                ))
                                                .color(egui::Color32::from_rgb(134, 239, 172))
                                                .size(11.0),
                                            );
                                        });
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
                        for i in 0..self.rules.len() {
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
            // VALIDATION: Check for DLsite rule without metadata
            // ════════════════════════════════════════════════════════════════
            let is_dlsite_rule = self
                .rules
                .get(self.selected_rule_index)
                .map(|r| r.category.eq_ignore_ascii_case("dlsite"))
                .unwrap_or(false);

            let missing_metadata = is_dlsite_rule && self.metadata.is_none();
            
            tracing::debug!(
                "OrganizePanel render: is_dlsite_rule={}, metadata.is_none()={}, missing_metadata={}",
                is_dlsite_rule,
                self.metadata.is_none(),
                missing_metadata
            );

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
                OrganizeTab::Preview => self.render_preview_tab(ui, &mut action, missing_metadata),
                OrganizeTab::NetworkActivity => self.render_network_tab(ui),
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

    fn render_preview_tab(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<OrganizePanelAction>,
        missing_metadata: bool,
    ) {
        if missing_metadata {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                // Large X Icon
                ui.label(
                    egui::RichText::new(egui_phosphor::regular::X)
                        .size(120.0)
                        .color(egui::Color32::from_rgb(100, 100, 100)), // Darker grey for the icon
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
            return;
        }

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
                                // Export logic
                                let filtered_tree =
                                    crate::shared::components::preview_tree::filter_tree(
                                        &self.organized_tree,
                                        self.preview_filter,
                                    );
                                if let Ok(json) = serde_json::to_string_pretty(&filtered_tree) {
                                    // Save to file dialog? Or just dump to clipboard/file?
                                    // Implementation: Write to "tree_export.json" in temp or current dir for now?
                                    // Or use rfd? UI crate might probably not have filtering dialog.
                                    // Let's write to "preview_export.json" in current dir and notify.
                                    if let Err(e) = std::fs::write("preview_export.json", json) {
                                        tracing::error!("Failed to export tree: {}", e);
                                    } else {
                                        tracing::info!("Exported tree to preview_export.json");
                                    }
                                }
                            }
                        });
                    });
                });

            if missing_metadata {
                ui.add_enabled_ui(false, |ui| {
                    ui.add_space(4.0);
                    // ... Proceed to render tree but disabled ...
                });
                // Actually, we just want to disable the Apply button, specifically.
                // The user said "show... before you can organize, you can interact with organizer".
                // Interaction with organizer likely refers to tree view toggle etc?
                // Let's stick to warning + apply disabled.
            }

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
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Template:").weak().size(11.0));
                                ui.label(
                                    RichText::new(&plan.root_folder_template)
                                        .monospace()
                                        .size(11.0),
                                );
                            });
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
            // STATS BAR
            // ════════════════════════════════════════════════════════════════
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {} files",
                        egui_phosphor::regular::FILE,
                        plan.moves.len()
                    ))
                    .size(11.0)
                    .weak(),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} {} generated",
                        egui_phosphor::regular::SPARKLE,
                        plan.generated_files.len()
                    ))
                    .size(11.0)
                    .weak(),
                );
                if !plan.downloads.is_empty() {
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} {} downloads",
                            egui_phosphor::regular::DOWNLOAD,
                            plan.downloads.len()
                        ))
                        .size(11.0)
                        .weak(),
                    );

                    if !self.is_loading_screenshots {
                        ui.separator();
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
                        ui.separator();
                        ui.spinner();
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
}
