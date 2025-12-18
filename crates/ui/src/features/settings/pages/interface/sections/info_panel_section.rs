//! Info panel settings section - configure which property groups are displayed.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_db::{UiItem, UiRegion};
use arclain_widgets::ToggleSwitch;
use eframe::egui;

/// Render the info panel configuration section
pub fn render(ui: &mut egui::Ui, theme: &AppTheme, items: &mut Vec<UiItem>, on_change: &mut bool) {
    SectionHeader::new("Property Groups").show(ui, &theme.colors);

    ui.label(
        egui::RichText::new("Choose which sections appear in the properties panel")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    // Filter and sort info panel sections
    let mut info_items: Vec<&mut UiItem> = items
        .iter_mut()
        .filter(|i| i.region == UiRegion::InfoPanel)
        .collect();
    info_items.sort_by_key(|i| i.sort_order);

    for item in info_items.iter_mut() {
        let label = item.label.clone();
        SettingsRow::new(&label)
            .description(format!("Show the {} section", label))
            .action(|ui| {
                if ui
                    .add(ToggleSwitch::new(&mut item.visible).size(44.0, 22.0))
                    .changed()
                {
                    *on_change = true;
                }
            })
            .show(ui, &theme.colors);
    }
}
