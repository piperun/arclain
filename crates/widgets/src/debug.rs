//! Visual debug overlay helpers for widget rendering.
//!
//! When a widget renders incorrectly — text in the wrong place, rect
//! the wrong size, child not centered in parent — guessing at the
//! cause wastes iteration. These helpers paint the actual numbers on
//! screen so you can SEE what's going on.
//!
//! Use cases:
//!
//!  * **Widget rect**: where did this widget actually allocate? Is
//!    its size what I thought? `paint_widget_rect_debug` shows the
//!    outline, center cross, and dimensions/position label.
//!
//!  * **Text centering**: a widget paints text inside a container.
//!    Is the visible-glyph midline coinciding with the container
//!    center? `paint_text_centering_debug` shows the container
//!    center, the galley's mesh_bounds (actual painted-glyph
//!    bounds), the delta between them, and a labeled outline so you
//!    can read the offset directly off the screen.
//!
//!  * **Parent-relative positioning**: where is this child within
//!    its parent rect? `paint_child_in_parent_debug` shows both
//!    rects with arrows for the gap on each side, so you can
//!    confirm a child is centered, padded, etc.
//!
//! # Toggling overlays
//!
//! All helpers take an `enabled: bool` so callers can route the
//! decision however they want. The canonical project-wide toggle is
//! the `EGUI_UI_DEBUG_GUIDELINES` env var, exposed via
//! `ui_debug_guidelines_enabled()`. Setting it to "1" / "true" /
//! "yes" / "on" before launch turns on every overlay that opts in.
//! Per-widget builder switches (`Chips::debug_lines(true)`, …) still
//! work and OR with the env flag, so you can pin one widget on for a
//! local debug session without turning on the rest of the UI.
//!
//! Future widgets that hit a "where exactly does this thing land"
//! question should reach for these instead of inventing their own
//! debug paint code — uniform colors and labels make it easy to
//! compare debug overlays across widgets in screenshots.

use egui::{Align2, Color32, FontId, Galley, Painter, Pos2, Rect, Stroke, Vec2};
use std::sync::{Arc, OnceLock};

/// Project-wide debug-overlay toggle, gated by the
/// `EGUI_UI_DEBUG_GUIDELINES` env var.
///
/// Reads the env var once on first call and caches the result via
/// `OnceLock` — subsequent calls are a single atomic load. Recognised
/// truthy values (case-insensitive): `1`, `true`, `yes`, `on`.
/// Anything else (including unset) is false.
///
/// Widgets that paint debug guidelines should OR this against any
/// per-widget builder switch they expose, so a global toggle lights
/// up every widget while local switches still work for focused
/// sessions.
pub fn ui_debug_guidelines_enabled() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("EGUI_UI_DEBUG_GUIDELINES")
            .ok()
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                matches!(v.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false)
    })
}

/// Paint a small frame-time / FPS HUD in the top-right corner and
/// enable egui's built-in `debug_on_hover` (hovering any widget shows
/// its layout rect in egui's own overlay, complementary to the
/// per-widget guidelines this crate paints). Both effects gated by
/// the same `EGUI_UI_DEBUG_GUIDELINES` env var.
///
/// Call once per frame from the top-level app update loop, **after**
/// the main UI render so the HUD sits on top. Cheap when the env var
/// is unset (single atomic load, early return).
pub fn paint_global_debug_hud(ctx: &egui::Context) {
    if !ui_debug_guidelines_enabled() {
        return;
    }

    // egui's own debug-on-hover. Re-set every frame is fine — it's a
    // single style-field write, and lets the env var stay the single
    // source of truth even if other code later toggles the flag.
    ctx.style_mut(|style| {
        style.debug.debug_on_hover = true;
    });

    let dt = ctx.input(|i| i.unstable_dt);
    let frame_ms = dt * 1000.0;
    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
    let text = format!("{:>5.1} ms · {:>3.0} fps", frame_ms, fps);

    egui::Area::new(egui::Id::new("arclain_debug_hud"))
        .anchor(Align2::RIGHT_TOP, Vec2::new(-8.0, 8.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(Color32::from_black_alpha(200))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(80, 200, 80)))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .size(11.0)
                            .color(Color32::from_rgb(120, 255, 160))
                            .monospace(),
                    );
                });
        });
}

/// Color palette used by every debug helper. Pick one role per
/// channel so a screenshot is unambiguous regardless of which
/// helper produced it.
pub mod debug_colors {
    use egui::Color32;
    pub const RECT_OUTLINE: Color32 = Color32::MAGENTA;
    pub const RECT_CENTER: Color32 = Color32::from_rgb(255, 80, 80);
    pub const TEXT_BOUNDS: Color32 = Color32::YELLOW;
    pub const TEXT_CENTER: Color32 = Color32::from_rgb(0, 220, 220);
    pub const PARENT: Color32 = Color32::from_rgb(180, 180, 180);
    pub const CHILD: Color32 = Color32::from_rgb(80, 200, 255);
    pub const GAP_LINE: Color32 = Color32::from_rgb(0, 200, 0);
    pub const LABEL_BG: Color32 = Color32::from_black_alpha(180);
}

/// Paint a debug outline + center cross + label for a widget's rect.
///
/// Useful when you suspect the widget is the wrong size, in the wrong
/// position, or being clipped by a parent. The label shows the rect's
/// position (top-left) and size, plus an optional caller-supplied tag
/// to identify which widget you're looking at when several are on
/// screen at once.
pub fn paint_widget_rect_debug(painter: &Painter, rect: Rect, label: &str, enabled: bool) {
    if !enabled {
        return;
    }

    // Rect outline.
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        Stroke::new(1.0_f32, debug_colors::RECT_OUTLINE),
        egui::StrokeKind::Middle,
    );

    // Center cross — short horizontal + vertical lines through center
    // so you can read the geometric center even when the widget's
    // content overlaps the rect outline.
    let c = rect.center();
    let half = 6.0;
    let cross = Stroke::new(1.0_f32, debug_colors::RECT_CENTER);
    painter.line_segment(
        [Pos2::new(c.x - half, c.y), Pos2::new(c.x + half, c.y)],
        cross,
    );
    painter.line_segment(
        [Pos2::new(c.x, c.y - half), Pos2::new(c.x, c.y + half)],
        cross,
    );

    // Label: "<tag> 123,45 80×24" at top-left, anchored just outside
    // the rect so it doesn't sit on top of the widget's content.
    paint_label_chip(
        painter,
        Pos2::new(rect.left(), rect.top() - 2.0),
        Align2::LEFT_BOTTOM,
        &format!(
            "{}  {:.0},{:.0}  {:.0}×{:.0}",
            label,
            rect.left(),
            rect.top(),
            rect.width(),
            rect.height()
        ),
    );
}

/// Paint debug overlay comparing inner content's painted bounds
/// against its container rect's geometric center.
///
/// Use this whenever you need to verify some inner content is
/// *visually* centered inside its container — text galleys (pass
/// `mesh_bounds` translated to screen coords), but also any other
/// "where did this thing actually paint" question. The label shows
/// the (dx, dy) offset between inner-rect center and container
/// center; (0, 0) means perfect alignment.
///
///   ┌────────────────────────────┐  <- container (magenta)
///   │   ┌── inner rect ────────┐ │  <- inner   (yellow)
///   │   │   ✕ <- inner ctr ────┼─│──── magenta cross at container ctr
///   │   └──────────────────────┘ │
///   └────────────────────────────┘
///
/// For text: pass the screen-space rect computed from
/// `galley_origin + galley.mesh_bounds.min` and `galley.mesh_bounds.size()`.
/// `widgets::paint_text_left_in_rect_visually_centered` already
/// returns exactly that rect.
pub fn paint_centering_debug(
    painter: &Painter,
    container: Rect,
    inner: Rect,
    label: &str,
    enabled: bool,
) {
    if !enabled {
        return;
    }

    // Container outline + center cross.
    painter.rect_stroke(
        container,
        egui::CornerRadius::ZERO,
        Stroke::new(1.0_f32, debug_colors::RECT_OUTLINE),
        egui::StrokeKind::Middle,
    );
    let cc = container.center();
    let cross = Stroke::new(1.0_f32, debug_colors::RECT_CENTER);
    painter.line_segment(
        [Pos2::new(cc.x - 8.0, cc.y), Pos2::new(cc.x + 8.0, cc.y)],
        cross,
    );
    painter.line_segment(
        [Pos2::new(cc.x, cc.y - 8.0), Pos2::new(cc.x, cc.y + 8.0)],
        cross,
    );

    // Inner outline + center cross.
    painter.rect_stroke(
        inner,
        egui::CornerRadius::ZERO,
        Stroke::new(1.0_f32, debug_colors::TEXT_BOUNDS),
        egui::StrokeKind::Middle,
    );
    let mc = inner.center();
    let cross = Stroke::new(1.0_f32, debug_colors::TEXT_CENTER);
    painter.line_segment(
        [Pos2::new(mc.x - 6.0, mc.y), Pos2::new(mc.x + 6.0, mc.y)],
        cross,
    );
    painter.line_segment(
        [Pos2::new(mc.x, mc.y - 6.0), Pos2::new(mc.x, mc.y + 6.0)],
        cross,
    );

    // Label: offset between inner center and container center.
    let dx = mc.x - cc.x;
    let dy = mc.y - cc.y;
    paint_label_chip(
        painter,
        Pos2::new(container.right() + 4.0, container.center().y),
        Align2::LEFT_CENTER,
        &format!("{}  Δ {:+.1},{:+.1}", label, dx, dy),
    );
}

/// Convenience: text variant of `paint_centering_debug` that takes
/// the galley + its screen origin directly. Computes mesh-bounds
/// rect for you. Use when you have a galley you just painted but
/// not a precomputed mesh rect.
pub fn paint_text_centering_debug(
    painter: &Painter,
    container: Rect,
    galley_origin: Pos2,
    galley: &Arc<Galley>,
    label: &str,
    enabled: bool,
) {
    let mesh = galley.mesh_bounds;
    let mesh_screen = Rect::from_min_size(galley_origin + mesh.min.to_vec2(), mesh.size());
    paint_centering_debug(painter, container, mesh_screen, label, enabled);
}

/// Paint debug overlay showing a child rect's gaps to its parent
/// rect's edges. Helps verify centering / padding visually.
pub fn paint_child_in_parent_debug(
    painter: &Painter,
    parent: Rect,
    child: Rect,
    label: &str,
    enabled: bool,
) {
    if !enabled {
        return;
    }

    painter.rect_stroke(
        parent,
        egui::CornerRadius::ZERO,
        Stroke::new(1.0_f32, debug_colors::PARENT),
        egui::StrokeKind::Middle,
    );
    painter.rect_stroke(
        child,
        egui::CornerRadius::ZERO,
        Stroke::new(1.0_f32, debug_colors::CHILD),
        egui::StrokeKind::Middle,
    );

    let gap_stroke = Stroke::new(1.0_f32, debug_colors::GAP_LINE);
    let mid_y = child.center().y;
    let mid_x = child.center().x;

    // Left gap arrow.
    painter.line_segment(
        [
            Pos2::new(parent.left(), mid_y),
            Pos2::new(child.left(), mid_y),
        ],
        gap_stroke,
    );
    // Right.
    painter.line_segment(
        [
            Pos2::new(child.right(), mid_y),
            Pos2::new(parent.right(), mid_y),
        ],
        gap_stroke,
    );
    // Top.
    painter.line_segment(
        [
            Pos2::new(mid_x, parent.top()),
            Pos2::new(mid_x, child.top()),
        ],
        gap_stroke,
    );
    // Bottom.
    painter.line_segment(
        [
            Pos2::new(mid_x, child.bottom()),
            Pos2::new(mid_x, parent.bottom()),
        ],
        gap_stroke,
    );

    let left_gap = child.left() - parent.left();
    let right_gap = parent.right() - child.right();
    let top_gap = child.top() - parent.top();
    let bottom_gap = parent.bottom() - child.bottom();
    paint_label_chip(
        painter,
        Pos2::new(parent.right() + 4.0, parent.center().y),
        Align2::LEFT_CENTER,
        &format!(
            "{}  L{:.0} R{:.0} T{:.0} B{:.0}",
            label, left_gap, right_gap, top_gap, bottom_gap
        ),
    );
}

/// Tiny background-shaded label for debug overlays. Rendered on top
/// of whatever's underneath so debug text stays legible against any
/// widget background.
fn paint_label_chip(painter: &Painter, pos: Pos2, anchor: Align2, text: &str) {
    let font = FontId::proportional(10.0);
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let size = galley.size();
    let rect_pos = match anchor {
        Align2::LEFT_BOTTOM => Pos2::new(pos.x, pos.y - size.y),
        Align2::LEFT_CENTER => Pos2::new(pos.x, pos.y - size.y * 0.5),
        Align2::LEFT_TOP => pos,
        _ => pos,
    };
    let bg_rect = Rect::from_min_size(rect_pos, size).expand2(Vec2::new(3.0, 1.0));
    painter.rect_filled(bg_rect, egui::CornerRadius::same(2), debug_colors::LABEL_BG);
    painter.galley(rect_pos, galley, Color32::WHITE);
}
