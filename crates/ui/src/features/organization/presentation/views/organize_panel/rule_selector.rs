use super::OrganizePanelAction;
use arclain_app::organization::OrganizationRuleSummary;
use arclain_widgets::ThemedDropdown;
use eframe::egui;

pub fn render_rule_selector(
    ui: &mut egui::Ui,
    archive_name: &str,
    rules: &[OrganizationRuleSummary],
    selected_rule_index: &mut usize,
) -> (Option<OrganizePanelAction>, bool) {
    let mut action = None;
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(egui_phosphor::regular::FUNNEL).size(14.0));
        arclain_widgets::Text::new("Rule:").strong().show(ui);

        let current_rule = rules
            .get(*selected_rule_index)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "None".to_string());

        // Boundary note: detecting a DLsite product code in a file name
        // is the metadata feature's own vocabulary -- the same detector
        // the application uses to resolve library metadata -- not the
        // organization surface's, so it stays on its current path until
        // the metadata read model is designed. Everything else this
        // selector shows is facade data.
        let has_dlsite_code = arclain_core::utilities::has_dlsite_code(archive_name);

        ThemedDropdown::new("rule_selector", &current_rule)
            .width(200.0)
            .show_ui(ui, |ui| {
                let mut categories: std::collections::BTreeMap<String, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for (i, rule) in rules.iter().enumerate() {
                    let cat = rule
                        .trigger
                        .metadata_source
                        .clone()
                        .unwrap_or_else(|| "General".to_string());
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
                        let rule = &rules[i];
                        let is_dlsite_rule = rule
                            .trigger
                            .metadata_source
                            .as_deref()
                            .map(|s| s.eq_ignore_ascii_case("dlsite"))
                            .unwrap_or(false);
                        let is_disabled = is_dlsite_rule && !has_dlsite_code;

                        if is_disabled {
                            let label = format!("{} (no DLsite code)", rule.name);
                            ui.add_enabled(
                                false,
                                egui::Button::new(egui::RichText::new(label).weak())
                                    .selected(*selected_rule_index == i),
                            );
                        } else if ui
                            .selectable_value(selected_rule_index, i, &rule.name)
                            .changed()
                        {
                            changed = true;
                        }
                    }
                    ui.add_space(4.0);
                }
            });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .link(egui::RichText::new("Manage Rules...").size(11.0))
                .clicked()
            {
                action = Some(OrganizePanelAction::ManageRules);
            }
        });
    });

    (action, changed)
}
