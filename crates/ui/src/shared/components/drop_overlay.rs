//! Translucent overlay rendered during file drag-hover.
//!
//! Surfaces explicit drop zones for new-tab vs replace-current-tab so
//! users don't have to memorize modifier keys. Caller decides when to
//! show the overlay (typically when `ctx.input(|i| !i.raw.hovered_files
//! .is_empty())`).

use crate::core::tabs::TabsCollection;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    NewTab,
    ReplaceCurrent,
}

/// Render the drop overlay. `drop_pos` is the current cursor position
/// during drag-hover (typically `ui.input(|i| i.pointer.hover_pos())`).
///
/// Returns `Some(zone)` when `drop_pos` falls inside a zone — the caller
/// uses this to route the file. Returns `None` if outside either zone
/// or when `drop_pos` is None. When the active tab has no archive_path,
/// only the "Open as new tab" zone is rendered (replace would be a no-op).
pub fn render_drop_overlay(
    ui: &mut egui::Ui,
    col: &TabsCollection,
    drop_pos: Option<egui::Pos2>,
) -> Option<DropZone> {
    let area_rect = ui.available_rect_before_wrap();
    let translucent_bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180);
    ui.painter().rect_filled(area_rect, 0.0, translucent_bg);

    let active_has_archive = col.active().archive_path.get().is_some();
    let mut routed: Option<DropZone> = None;

    let zone_height = 180.0;
    let zone_width = 320.0;
    let gap = 24.0;
    let total_width = if active_has_archive {
        zone_width * 2.0 + gap
    } else {
        zone_width
    };
    let start_x = area_rect.center().x - total_width / 2.0;
    let zone_y = area_rect.center().y - zone_height / 2.0;

    let new_tab_rect = egui::Rect::from_min_size(
        egui::pos2(start_x, zone_y),
        egui::vec2(zone_width, zone_height),
    );
    draw_zone(ui, new_tab_rect, "Open as new tab", true, None);

    if active_has_archive {
        let replace_rect = egui::Rect::from_min_size(
            egui::pos2(start_x + zone_width + gap, zone_y),
            egui::vec2(zone_width, zone_height),
        );
        // Ctrl-held drops always route to Replace regardless of cursor
        // zone — surface that here so users don't need to discover it.
        draw_zone(
            ui,
            replace_rect,
            "Replace current tab",
            false,
            Some("Hold Ctrl"),
        );

        if let Some(pos) = drop_pos {
            if new_tab_rect.contains(pos) {
                routed = Some(DropZone::NewTab);
            } else if replace_rect.contains(pos) {
                routed = Some(DropZone::ReplaceCurrent);
            }
        }
    } else if let Some(pos) = drop_pos {
        if new_tab_rect.contains(pos) {
            routed = Some(DropZone::NewTab);
        }
    }

    routed
}

fn draw_zone(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    label: &str,
    primary: bool,
    hotkey_hint: Option<&str>,
) {
    let fill = if primary {
        egui::Color32::from_rgba_unmultiplied(0, 120, 215, 200)
    } else {
        egui::Color32::from_rgba_unmultiplied(60, 60, 60, 200)
    };
    ui.painter().rect_filled(rect, 12.0, fill);

    // Main label — render as a widget so egui_kittest can query it.
    // When a hotkey hint is present, shift the label slightly up so the
    // hint can sit centered below it without overlapping.
    let label_inset = if hotkey_hint.is_some() {
        let mut r = rect.shrink(20.0);
        r.max.y -= 36.0;
        r
    } else {
        rect.shrink(20.0)
    };
    ui.put(
        label_inset,
        egui::Label::new(
            egui::RichText::new(label)
                .size(20.0)
                .color(egui::Color32::WHITE),
        ),
    );

    // Hotkey hint pill — small, dimmer, sits below the main label.
    // Rendered as a widget (not just painter text) so kittest can
    // verify the "Hold Ctrl" affordance is actually wired.
    if let Some(hint) = hotkey_hint {
        let hint_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 20.0, rect.max.y - 56.0),
            egui::vec2(rect.width() - 40.0, 28.0),
        );
        ui.put(
            hint_rect,
            egui::Label::new(
                egui::RichText::new(hint)
                    .size(13.0)
                    .color(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
            ),
        );
    }
}
