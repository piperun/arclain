//! Info panel settings section - visibility toggles for info-panel sections.
//!
//! Reads `shared.signals().info_panel_items` (the canonical source)
//! and returns at most one `(item_id, new_visible)` event per render
//! frame. The caller wraps that into an
//! `InterfaceSettingsAction::ToggleItemVisibility` and the dispatcher
//! is the only place that mutates the DB or the signal.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_app::layout::UiItemDto;
use arclain_widgets::ToggleSwitch;
use eframe::egui;

/// Render the info panel configuration section. Returns
/// `Some((item_id, new_visible))` if the user toggled a row this frame.
pub fn render(ui: &mut egui::Ui, theme: &AppTheme, items: &[UiItemDto]) -> Option<(String, bool)> {
    SectionHeader::new("Property Groups").show(ui, &theme.colors);

    ui.label(
        egui::RichText::new("Choose which sections appear in the properties panel")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    let mut sorted_items: Vec<&UiItemDto> = items.iter().collect();
    sorted_items.sort_by_key(|i| i.sort_order);

    let mut emitted: Option<(String, bool)> = None;

    for item in sorted_items {
        let label = item.label.clone();
        let mut visible = item.visible;

        SettingsRow::new(&label)
            .description(format!("Show the {} section", label))
            .action(|ui| {
                if ui
                    .add(ToggleSwitch::new(&mut visible).size(44.0, 22.0))
                    .changed()
                    && emitted.is_none()
                {
                    emitted = Some((item.id.clone(), visible));
                }
            })
            .show(ui, &theme.colors);
    }

    emitted
}
