use crate::shared::components::network_log::NetworkLog;
use arclain_core::organization::{engine::RuleEngine, OrganizationRule};
use arclain_core::ArchiveEntry;
use eframe::egui;

#[derive(Default, PartialEq, Clone, Copy)]
pub enum OrganizeTab {
    #[default]
    Preview,
    NetworkActivity,
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
            rules,
            selected_rule_index: 0,
            preview_plan: None,
            metadata,
            network_log: Vec::new(),
            active_tab: OrganizeTab::Preview,
        };
        panel.update_preview();
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
                self.preview_plan = Some(plan);
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

    fn filename(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or(path)
    }

    fn directory(path: &str) -> &str {
        match path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        }
    }

    pub fn render(&mut self, ctx: &egui::Context) -> Option<bool> {
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
                        action = Some(false);
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
                        action = Some(true);
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

                egui::ComboBox::from_id_salt("rule_selector")
                    .selected_text(&current_rule)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for i in 0..self.rules.len() {
                            let rule = &self.rules[i];
                            if ui
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
            });

            ui.separator();

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
                OrganizeTab::Preview => self.render_preview_tab(ui),
                OrganizeTab::NetworkActivity => self.render_network_tab(ui),
            }
        });

        action
    }

    fn render_preview_tab(&self, ui: &mut egui::Ui) {
        if let Some(plan) = &self.preview_plan {
            // Output folder
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(35, 45, 55))
                .inner_margin(10.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::FOLDER)
                                .color(egui::Color32::from_rgb(250, 204, 21)),
                        );
                        ui.label(egui::RichText::new("Output:").strong());
                        ui.label(
                            egui::RichText::new(&plan.root_folder)
                                .monospace()
                                .color(egui::Color32::from_rgb(147, 197, 253)),
                        );
                    });
                });

            ui.add_space(4.0);

            // File count header
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(egui_phosphor::regular::FILES).size(14.0));
                ui.label(egui::RichText::new("File Operations").strong());
                ui.label(
                    egui::RichText::new(format!("({} files)", plan.moves.len()))
                        .weak()
                        .size(11.0),
                );
            });

            // Scrollable file list
            egui::ScrollArea::both()
                .id_salt("preview_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 3.0);

                    for (src, dst) in &plan.moves {
                        let src_file = Self::filename(src);
                        let src_dir = Self::directory(src);
                        let dst_dir = Self::directory(dst);

                        egui::Frame::NONE
                            .fill(ui.style().visuals.faint_bg_color)
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .corner_radius(3.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(egui_phosphor::regular::FILE)
                                            .size(12.0)
                                            .color(egui::Color32::from_rgb(156, 163, 175)),
                                    );
                                    ui.label(egui::RichText::new(src_file).strong().size(12.0));
                                });
                                ui.horizontal(|ui| {
                                    ui.add_space(18.0);
                                    let src_display = if src_dir.is_empty() {
                                        "(root)".to_string()
                                    } else {
                                        Self::truncate_path(src_dir, 35)
                                    };
                                    ui.label(
                                        egui::RichText::new(src_display)
                                            .weak()
                                            .size(10.0)
                                            .monospace(),
                                    );
                                    ui.label(
                                        egui::RichText::new(egui_phosphor::regular::ARROW_RIGHT)
                                            .size(10.0)
                                            .color(egui::Color32::from_rgb(74, 222, 128)),
                                    );
                                    let dst_display = if dst_dir.is_empty() {
                                        "(root)".to_string()
                                    } else {
                                        Self::truncate_path(dst_dir, 35)
                                    };
                                    ui.label(
                                        egui::RichText::new(dst_display)
                                            .size(10.0)
                                            .monospace()
                                            .color(egui::Color32::from_rgb(147, 197, 253)),
                                    );
                                });
                            });
                    }

                    // Generated files
                    if !plan.generated_files.is_empty() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(egui_phosphor::regular::SPARKLE)
                                    .color(egui::Color32::from_rgb(250, 204, 21)),
                            );
                            ui.label(egui::RichText::new("Generated Files").strong());
                        });
                        for (path, _) in &plan.generated_files {
                            egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(55, 48, 35))
                                .inner_margin(6.0)
                                .corner_radius(3.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(egui_phosphor::regular::FILE_PLUS)
                                                .color(egui::Color32::from_rgb(250, 204, 21)),
                                        );
                                        ui.label(egui::RichText::new(path).monospace().size(11.0));
                                    });
                                });
                        }
                    }
                });
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new(egui_phosphor::regular::WARNING)
                        .size(40.0)
                        .color(egui::Color32::from_rgb(251, 191, 36)),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("No preview available")
                        .size(14.0)
                        .weak(),
                );
            });
        }
    }

    fn render_network_tab(&self, ui: &mut egui::Ui) {
        NetworkLog::render(ui, &self.network_log);
    }
}
