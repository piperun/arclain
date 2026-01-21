//! Context menu settings section - configure visibility and order of context menu items.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_core::{UiItem, UiRegion};
use arclain_widgets::ToggleSwitch;
use eframe::egui;

/// Render the context menu configuration section
pub fn render(ui: &mut egui::Ui, theme: &AppTheme, items: &mut Vec<UiItem>, on_change: &mut bool) {
    SectionHeader::new("Menu Items").show(ui, &theme.colors);

    ui.label(
        egui::RichText::new("Configure which items appear in the right-click context menu")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    // Filter and sort context menu items
    let mut context_items: Vec<&mut UiItem> = items
        .iter_mut()
        .filter(|i| i.region == UiRegion::ContextMenu)
        .collect();
    context_items.sort_by_key(|i| i.sort_order);

    for item in context_items.iter_mut() {
        let icon = item
            .icon
            .as_ref()
            .map(|n| icon_name_to_char(n))
            .unwrap_or("");
        let label = format!("{} {}", icon, item.label);

        SettingsRow::new(&label)
            .description(format!("Show '{}' in context menu", item.label))
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

/// Map icon name to Phosphor icon character
fn icon_name_to_char(name: &str) -> &'static str {
    match name {
        "FOLDER_OPEN" => egui_phosphor::regular::FOLDER_OPEN,
        "EXPORT" => egui_phosphor::regular::EXPORT,
        "COPY" => egui_phosphor::regular::COPY,
        "TRASH" => egui_phosphor::regular::TRASH,
        "INFO" => egui_phosphor::regular::INFO,
        "PENCIL" => egui_phosphor::regular::PENCIL,
        _ => egui_phosphor::regular::QUESTION,
    }
}
