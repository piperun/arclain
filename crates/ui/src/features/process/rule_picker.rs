//! Rule selector widget — lists the application's organization rules.

use arclain_app::organization::OrganizationRuleSummary;
use eframe::egui;

/// Render a combo box picker for organization rules.
/// `rules` should be pre-loaded by the caller.
/// `selected_id` is the numeric rule id a `PipelineStep::Organize`
/// carries; a rule whose id is not numeric (which the application never
/// produces) is simply not selectable.
/// Returns true if the selection changed.
pub fn render(
    ui: &mut egui::Ui,
    id_salt: &str,
    rules: &[OrganizationRuleSummary],
    selected_id: &mut i64,
) -> bool {
    let mut changed = false;

    let numeric_id = |rule: &OrganizationRuleSummary| rule.id.parse::<i64>().ok();

    let selected_label = rules
        .iter()
        .find(|r| numeric_id(r) == Some(*selected_id))
        .map(|r| r.name.clone())
        .unwrap_or_else(|| {
            if *selected_id == 0 {
                "— pick a rule —".to_string()
            } else {
                format!("Unknown rule #{}", selected_id)
            }
        });

    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            for rule in rules {
                let Some(id) = numeric_id(rule) else {
                    continue;
                };
                if ui.selectable_value(selected_id, id, &rule.name).clicked() {
                    changed = true;
                }
            }
            if rules.is_empty() {
                ui.label(
                    egui::RichText::new("No rules configured — see Settings > Organization Rules")
                        .weak(),
                );
            }
        });

    changed
}
