use arclain_core::organization::{engine::RuleEngine, OrganizationRule};
use arclain_core::ArchiveEntry;
use eframe::egui;

pub struct OrganizePanel {
    pub archive_name: String,
    pub entries: Vec<ArchiveEntry>,
    pub rules: Vec<OrganizationRule>,
    pub selected_rule_index: usize,
    pub preview_plan: Option<arclain_core::organization::engine::OrganizationPlan>,
    pub metadata: Option<arclain_core::archive_organizer::GameMetadata>,
    pub network_log: Vec<(std::time::SystemTime, String)>,
}

impl OrganizePanel {
    pub fn new(
        archive_name: String,
        entries: Vec<ArchiveEntry>,
        rules: Vec<OrganizationRule>,
        metadata: Option<arclain_core::archive_organizer::GameMetadata>,
    ) -> Self {
        let mut panel = Self {
            archive_name: archive_name.clone(),
            entries: entries.clone(),
            rules,
            selected_rule_index: 0,
            preview_plan: None,
            metadata,
            network_log: Vec::new(),
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
            }
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        // Returns Some(true) if Apply clicked, Some(false) if Cancel, None otherwise
        let mut action = None;

        ui.heading(format!("Organize Archive: {}", self.archive_name));
        ui.add_space(10.0);

        // Rule Selector
        ui.horizontal(|ui| {
            ui.label("Select Rule:");
            let current_rule_name = self
                .rules
                .get(self.selected_rule_index)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "No matching rules".to_string());

            egui::ComboBox::from_id_salt("rule_selector")
                .selected_text(current_rule_name)
                .show_ui(ui, |ui| {
                    for i in 0..self.rules.len() {
                        let rule_name = self.rules[i].name.clone();
                        if ui
                            .selectable_value(&mut self.selected_rule_index, i, &rule_name)
                            .changed()
                        {
                            self.update_preview();
                        }
                    }
                });
        });

        ui.add_space(10.0);

        // Split view: Preview on left, Network Log on right (if any)
        ui.columns(2, |columns| {
            // Left Column: Preview
            columns[0].vertical(|ui| {
                ui.heading("Preview");
                if let Some(plan) = &self.preview_plan {
                    ui.label(format!("Root Folder: {}", plan.root_folder));
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .id_salt("preview_scroll")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for (src, dst) in &plan.moves {
                                ui.horizontal(|ui| {
                                    ui.label(src);
                                    ui.label("➡");
                                    ui.label(dst);
                                });
                            }

                            if !plan.generated_files.is_empty() {
                                ui.separator();
                                ui.label("Generated Files:");
                                for (path, _) in &plan.generated_files {
                                    ui.horizontal(|ui| {
                                        ui.label("✨");
                                        ui.label(path);
                                    });
                                }
                            }
                        });
                } else {
                    ui.label("No preview available");
                }
            });

            // Right Column: Network Log
            columns[1].vertical(|ui| {
                ui.heading("Network Activity");
                egui::ScrollArea::vertical()
                    .id_salt("network_log_scroll")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if self.network_log.is_empty() {
                            ui.label(egui::RichText::new("No activity yet").italics().weak());
                        } else {
                            for (time, msg) in &self.network_log {
                                let time_str = chrono::DateTime::<chrono::Local>::from(*time)
                                    .format("%H:%M:%S")
                                    .to_string();
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&time_str).weak().size(10.0));
                                    ui.label(egui::RichText::new(msg).size(11.0));
                                });
                            }
                        }
                    });
            });
        });

        ui.add_space(20.0);

        // Actions
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                action = Some(false);
            }
            if ui.button("Apply Organization").clicked() {
                action = Some(true);
            }
        });

        action
    }
}
