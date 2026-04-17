//! Rule selector widget — lists rules from the OrganizationService.

use arclain_core::OrganizationRule;
use eframe::egui;

/// Render a combo box picker for organization rules.
/// `rules` should be pre-loaded by the caller (typically from OrganizationService).
/// Returns true if the selection changed.
pub fn render(
    ui: &mut egui::Ui,
    id_salt: &str,
    rules: &[OrganizationRule],
    selected_id: &mut i64,
) -> bool {
    let mut changed = false;

    let selected_label = rules
        .iter()
        .find(|r| r.id == *selected_id)
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
                if ui
                    .selectable_value(selected_id, rule.id, &rule.name)
                    .clicked()
                {
                    changed = true;
                }
            }
            if rules.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "No rules configured — see Settings > Organization Rules",
                    )
                    .weak(),
                );
            }
        });

    changed
}
