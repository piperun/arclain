//! Rule selector widget — lists the application's organization rules.

use arclain_app::organization::OrganizationRuleSummary;
use eframe::egui;

/// Render a combo box picker for organization rules.
/// `rules` should be pre-loaded by the caller.
/// `selected_id` is the rule id a `PipelineStepDto::Organize` carries,
/// in the application's own decimal-string form — the same string
/// `OrganizationRuleSummary::id` reports, so no numeric round trip is
/// needed to match one against the other. Empty means "nothing picked
/// yet"; an id no longer in `rules` is shown as unknown rather than
/// silently reset, so a preset that names a deleted rule is visible
/// instead of appearing unconfigured.
/// Returns true if the selection changed.
pub fn render(
    ui: &mut egui::Ui,
    id_salt: &str,
    rules: &[OrganizationRuleSummary],
    selected_id: &mut String,
) -> bool {
    let mut changed = false;

    let selected_label = rules
        .iter()
        .find(|r| r.id == *selected_id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| {
            if selected_id.is_empty() {
                "— pick a rule —".to_string()
            } else {
                format!("Unknown rule #{}", selected_id)
            }
        });

    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            for rule in rules {
                if ui
                    .selectable_label(*selected_id == rule.id, &rule.name)
                    .clicked()
                {
                    *selected_id = rule.id.clone();
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
