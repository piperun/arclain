//! Profile Selector Component
//!
//! Dropdown for selecting archive profile when organizing.

use arclain_core::features::organization::ArchiveProfile;
use eframe::egui;

/// Render the profile selector dropdown.
/// Returns true if the selection changed.
pub fn render_profile_selector(
    ui: &mut egui::Ui,
    profiles: &[ArchiveProfile],
    selected_profile_index: &mut usize,
) -> bool {
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(egui_phosphor::regular::ARCHIVE).size(14.0));
        arclain_widgets::Text::new("Profile:").strong().show(ui);

        let current_profile = profiles
            .get(*selected_profile_index)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "None".to_string());

        egui::ComboBox::from_id_salt("profile_selector")
            .selected_text(&current_profile)
            .width(200.0)
            .show_ui(ui, |ui| {
                for (i, profile) in profiles.iter().enumerate() {
                    let label = format!(
                        "{} ({}, level {})",
                        profile.name,
                        profile.format.display_name(),
                        profile.compression_level
                    );

                    let mut response = ui.selectable_value(selected_profile_index, i, &label);

                    // Add tooltip with description
                    if let Some(desc) = &profile.description {
                        response = response.on_hover_text(desc);
                    }

                    if response.changed() {
                        changed = true;
                    }
                }
            });

        // Show format badge
        if let Some(profile) = profiles.get(*selected_profile_index) {
            ui.label(
                egui::RichText::new(format!(".{}", profile.format.extension()))
                    .size(11.0)
                    .family(egui::FontFamily::Monospace)
                    .color(ui.visuals().text_color().gamma_multiply(0.6)),
            );
        }
    });

    changed
}
