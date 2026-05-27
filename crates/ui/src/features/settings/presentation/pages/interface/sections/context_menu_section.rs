//! Context menu settings section - visibility toggles for context-menu items.
//!
//! Reads `shared.signals().context_menu_items` (the canonical source)
//! and returns at most one `(item_id, new_visible)` event per render
//! frame. The caller wraps that into an
//! `InterfaceSettingsAction::ToggleItemVisibility` and the dispatcher
//! is the only place that mutates the DB or the signal.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_core::UiItem;
use arclain_widgets::ToggleSwitch;
use eframe::egui;

/// Render the context menu configuration section. Items are sorted by
/// `sort_order` before rendering; the section never modifies the input
/// slice. Returns `Some((item_id, new_visible))` if the user toggled
/// a row this frame.
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    items: &[UiItem],
) -> Option<(String, bool)> {
    SectionHeader::new("Menu Items").show(ui, &theme.colors);

    ui.label(
        egui::RichText::new("Configure which items appear in the right-click context menu")
            .size(12.0)
            .color(theme.colors.on_surface_variant),
    );
    ui.add_space(8.0);

    let mut sorted_items: Vec<&UiItem> = items.iter().collect();
    sorted_items.sort_by_key(|i| i.sort_order);

    let mut emitted: Option<(String, bool)> = None;

    for item in sorted_items {
        let icon = item
            .icon
            .as_deref()
            .map(icon_name_to_char)
            .unwrap_or("");
        let label = format!("{} {}", icon, item.label);
        // Local mutable mirror of `visible` so the ToggleSwitch widget
        // (which takes `&mut bool`) doesn't fight the signal-owned
        // source of truth. The mutation is discarded after render; the
        // *event* (item id + new value) is what flows back to the
        // dispatcher.
        let mut visible = item.visible;

        SettingsRow::new(&label)
            .description(format!("Show '{}' in context menu", item.label))
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
