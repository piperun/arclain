//! Standardized text-centering helpers.
//!
//! egui centers text by line-box midpoint, but the line box includes
//! ascender + descender slack that's not actually painted for typical
//! Latin text. So `Align2::CENTER_CENTER` and similar APIs make text
//! appear high inside tight containers (chips, badges, small buttons).
//!
//! `Galley::mesh_bounds` is the bounding box of the **actual painted
//! glyphs** — egui computes this when laying the text out. Centering
//! by `mesh_bounds.center()` puts the visible pixels on the target
//! center point, regardless of font ascender/descender quirks.
//!
//! Use these helpers any time you're painting text inside a
//! visually-bounded container (pill, badge, button) and you want it
//! to look centered to the eye.

use egui::{Color32, FontId, Painter, Pos2, Rect, Vec2};
use std::sync::Arc;

/// Layout `text` and return the galley plus the position at which to
/// paint it so its visible-glyph center lands on `target_center`.
///
/// Uses the painter's font cache. Safe to call from a Widget impl;
/// the returned origin is in the painter's coordinate space.
pub fn layout_text_visually_centered(
    painter: &Painter,
    text: impl Into<String>,
    font_id: FontId,
    color: Color32,
    target_center: Pos2,
) -> (Arc<egui::Galley>, Pos2) {
    let galley = painter.layout_no_wrap(text.into(), font_id, color);
    let mesh_center = galley.mesh_bounds.center();
    let origin = target_center - mesh_center.to_vec2();
    (galley, origin)
}

/// Convenience: lay out the text and paint it visually-centered on
/// `target_center` with `color`.
pub fn paint_text_visually_centered(
    painter: &Painter,
    text: impl Into<String>,
    font_id: FontId,
    color: Color32,
    target_center: Pos2,
) {
    let (galley, origin) =
        layout_text_visually_centered(painter, text, font_id, color, target_center);
    painter.galley(origin, galley, color);
}

/// Layout text and paint it visually-centered inside `rect`, with the
/// glyphs left-anchored at `rect.left() + h_pad`. Vertical centering
/// uses `mesh_bounds`. Useful for pills/chips where the text starts
/// at a fixed left margin but should look vertically centered.
pub fn paint_text_left_in_rect_visually_centered(
    painter: &Painter,
    text: impl Into<String>,
    font_id: FontId,
    color: Color32,
    rect: Rect,
    h_pad: f32,
) -> Rect {
    let galley = painter.layout_no_wrap(text.into(), font_id, color);
    let mesh = galley.mesh_bounds;
    // Vertical: align mesh_bounds.center().y to rect.center().y
    // Horizontal: align mesh_bounds.left() to rect.left() + h_pad
    let target_center_y = rect.center().y;
    let origin = Pos2 {
        x: (rect.left() + h_pad) - mesh.left(),
        y: target_center_y - mesh.center().y,
    };
    painter.galley(origin, galley.clone(), color);

    // Return the screen-space rect of the painted glyphs (useful for
    // debug overlays and positioning sibling content).
    Rect::from_min_size(origin + mesh.min.to_vec2(), Vec2::new(mesh.width(), mesh.height()))
}
