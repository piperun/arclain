//! Horizontal tab bar for multi-archive support.
//!
//! Each tab renders as a single pill: title text on the left, close (X)
//! icon on the right, both inside one visual unit. Clicking the body
//! switches; clicking the X closes. Text is visually centered via the
//! shared mesh-bounds helper from `arclain_widgets::text_layout` so
//! glyph ascenders/descenders don't make the title drift up/down as
//! more decorations are added.
//!
//! Layout:
//!
//! ```text
//! +-----------------------------------------------+
//! | [tab1] [tab2 active] [tab3] ...   | [⌄] [+]  |  <- tab strip
//! +-----------------------------------------------+
//! | ════▭▭▭▭▭═══════════════════════════════════ |  <- scrollbar strip
//! +-----------------------------------------------+
//! ```
//!
//! The `+` button is pinned to the right edge outside the scroll area
//! so it stays reachable regardless of how many tabs you have open.
//! Vertical mouse-wheel over the tab strip scrolls horizontally.
//! Switching the active tab (via click or keyboard) scrolls it into
//! view if it was off-screen.
//!
//! When tabs overflow horizontally, a draggable scrollbar renders in
//! its OWN strip directly below the tab row (rendered as a separate
//! panel by the caller — see `render_tab_scrollbar`) so it never eats
//! into the tab chips' height. The strip is omitted entirely when the
//! tabs fit. Dragging the thumb scrubs the strip; wheel scroll and
//! scroll-into-view keep working alongside it.

use crate::core::tabs::{TabId, TabsCollection};
use arclain_theme::ThemeColors;
use arclain_widgets::text_layout::{
    paint_text_left_in_rect_visually_centered, paint_text_visually_centered,
};
use eframe::egui;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarAction {
    Switch(TabId),
    Close(TabId),
    OpenEmpty,
    /// User dropped a tab at a new position. Indices are into the current
    /// `TabsCollection::tabs()` slice; the caller forwards to
    /// `TabsCollection::reorder`.
    Reorder {
        from_idx: usize,
        to_idx: usize,
    },
    /// Close every tab except the given one. Tabs with in-flight ops
    /// are skipped silently — user can close those individually.
    CloseOthers(TabId),
    /// Close every tab to the right of the given one. Same in-flight
    /// skip semantics as `CloseOthers`.
    CloseToRight(TabId),
    /// Open a new tab loading the same archive_path as the given tab.
    /// No-op if the source tab has no archive loaded.
    Duplicate(TabId),
    /// Pin or unpin the given tab. Pinned tabs sort to the front and
    /// are excluded from bulk-close actions.
    SetPinned(TabId, bool),
}

/// Payload stashed in `egui::DragAndDrop` while a tab chip is being
/// dragged. Kept private so other UI components don't accidentally
/// pick it up.
#[derive(Debug, Clone, Copy)]
struct TabDragPayload {
    from_idx: usize,
    from_id: TabId,
}

const TAB_HEIGHT: f32 = 26.0;
const TAB_H_PAD: f32 = 12.0;
const TAB_CLOSE_GAP: f32 = 8.0;
const TAB_CLOSE_HIT_SIZE: f32 = 18.0;
const TAB_GAP: f32 = 4.0;
const FONT_SIZE: f32 = 12.0;
/// Maximum width in pixels for the title text portion of a tab chip
/// when the tab is **not** active. Inactive tabs stay compact so more
/// fit before scroll kicks in.
const TAB_TITLE_MAX_WIDTH_INACTIVE: f32 = 180.0;
/// Maximum width for the **active** tab. Wider than inactive so the
/// currently-open archive shows a longer prefix at a glance. Chrome-
/// style adaptive width — switching tabs reflows the strip.
const TAB_TITLE_MAX_WIDTH_ACTIVE: f32 = 360.0;
/// Corner radius for tab chips and the `+` button. Currently 0 to match
/// arclain's brutalist visual language; once a theme system exposes a
/// `tab_corner_radius` variable, this becomes theme-driven.
const TAB_CORNER_RADIUS: u8 = 0;

/// Thickness of the draggable scrollbar track in its own strip below
/// the tab row. Taller than the old 5px indicator pill so the thumb is
/// an easy grab target.
const SCROLLBAR_THICKNESS: f32 = 8.0;
/// Total vertical space the scrollbar strip panel should reserve
/// (track + a little breathing room above/below). The caller sizes its
/// dedicated panel to this.
pub const SCROLLBAR_STRIP_HEIGHT: f32 = SCROLLBAR_THICKNESS + 6.0;

/// Scroll geometry captured during `render_tab_bar`, handed back so the
/// caller can render the scrollbar in its own panel below the tabs.
/// `has_overflow` gates whether that panel is shown at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct TabScrollInfo {
    pub offset: f32,
    pub content_width: f32,
    pub viewport_width: f32,
}

impl TabScrollInfo {
    pub fn has_overflow(&self) -> bool {
        (self.content_width - self.viewport_width) > 0.5
    }
}

/// Shared memory key: the scrollbar drag stashes its requested offset
/// here, and `render_tab_bar` reads it the next frame to drive the
/// ScrollArea via `horizontal_scroll_offset`. Both must agree on the
/// id, hence the helper.
fn pill_offset_key() -> egui::Id {
    egui::Id::new("tab_bar/pill_drag_offset")
}

/// Render the multi-archive tab bar (the tab chips only). Returns the
/// user action (if any) plus the scroll geometry so the caller can
/// render the scrollbar in its own strip below — see
/// `render_tab_scrollbar`.
pub fn render_tab_bar(
    ui: &mut egui::Ui,
    col: &TabsCollection,
    theme: &ThemeColors,
) -> (Option<TabBarAction>, TabScrollInfo) {
    let mut action: Option<TabBarAction> = None;

    // Track which tab was active last frame so we know when the user
    // switched, so we can scroll the new active tab into view (C).
    let last_active_id_key = egui::Id::new("multi_archive_tab_bar/last_active");
    let last_active: Option<TabId> = ui.memory(|m| m.data.get_temp(last_active_id_key));
    let current_active = col.active_id();
    let active_changed = last_active != Some(current_active);
    if active_changed {
        ui.memory_mut(|m| m.data.insert_temp(last_active_id_key, current_active));
    }

    // Captured for the position pill after the inner layout finishes.
    let mut scroll_offset_x: f32 = 0.0;
    let mut content_width: f32 = 0.0;
    let mut viewport_rect = egui::Rect::NOTHING;

    // A scrollbar drag last frame requested an explicit scroll offset.
    // The scrollbar lives in a separate panel rendered *after* this
    // one, so it can only feed the offset forward one frame.
    // `render_tab_scrollbar` writes this key while the thumb is grabbed
    // and clears it on release, so it only overrides the natural offset
    // during an active drag — wheel scroll and scroll-into-view keep
    // working untouched the rest of the time.
    let pending_pill_offset: Option<f32> = ui.memory(|m| m.data.get_temp(pill_offset_key()));

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // Row: scroll area on the left taking remaining width, then
        // chevron (overflow menu), then + button on the right edge.
        // right_to_left places items starting from the right, so plus
        // goes first and chevron sits immediately to its left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if render_plus_button(ui, theme).clicked() {
                action = Some(TabBarAction::OpenEmpty);
            }
            let chevron_resp = render_chevron_button(ui, theme);
            let popup_id = ui.make_persistent_id("tab_bar/overflow_popup");
            if chevron_resp.clicked() {
                egui::Popup::toggle_id(ui.ctx(), popup_id);
            }
            // Popup lists every tab — click to switch, X to close.
            // Renders as a stacked menu below the chevron. The Switch /
            // Close actions emitted here flow back through the same
            // `action` channel used by the chip strip.
            #[allow(deprecated)]
            egui::popup_below_widget(
                ui,
                popup_id,
                &chevron_resp,
                egui::PopupCloseBehavior::CloseOnClickOutside,
                |ui| {
                    ui.set_min_width(220.0);
                    for tab in col.tabs() {
                        let title = tab.display_title();
                        let is_active = tab.id == col.active_id();
                        ui.horizontal(|ui| {
                            // Selectable label is the row body — Chrome-style
                            // it's the click target for Switch.
                            if ui.selectable_label(is_active, &title).clicked() {
                                action = Some(TabBarAction::Switch(tab.id));
                                egui::Popup::close_id(ui.ctx(), popup_id);
                            }
                            // Right-aligned close button. Tap-and-hold the
                            // popup open: closing one tab doesn't dismiss
                            // the menu, so users can prune multiple in a
                            // row.
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button(egui_phosphor::regular::X)
                                        .on_hover_text("Close tab")
                                        .clicked()
                                    {
                                        action = Some(TabBarAction::Close(tab.id));
                                    }
                                },
                            );
                        });
                    }
                },
            );
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let mut scroll_area = egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
                if let Some(off) = pending_pill_offset {
                    scroll_area = scroll_area.horizontal_scroll_offset(off);
                }
                let scroll_out = scroll_area.show(ui, |ui| {
                    // (B) Vertical mouse wheel scrolls horizontally
                    // when pointer is over the tab strip.
                    let pointer_over = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| ui.max_rect().contains(p))
                        .unwrap_or(false);
                    if pointer_over {
                        ui.input_mut(|i| {
                            if i.raw_scroll_delta.x == 0.0 && i.raw_scroll_delta.y != 0.0 {
                                i.raw_scroll_delta.x = i.raw_scroll_delta.y;
                                i.raw_scroll_delta.y = 0.0;
                            }
                            if i.smooth_scroll_delta.x == 0.0 && i.smooth_scroll_delta.y != 0.0 {
                                i.smooth_scroll_delta.x = i.smooth_scroll_delta.y;
                                i.smooth_scroll_delta.y = 0.0;
                            }
                        });
                    }

                    // Use ui.horizontal (NOT horizontal_centered) — the
                    // latter grabs available_size_before_wrap() for its
                    // height, creating a feedback loop with the auto-
                    // sizing TopBottomPanel that explodes the tab strip
                    // to fill the entire window. ui.horizontal expands
                    // only to fit its content's natural height; cross-
                    // axis alignment is already Align::Center for
                    // left_to_right layout, so the chip pills are
                    // vertically centered relative to that height.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = TAB_GAP;

                        // Peek at the in-flight drag payload so chips
                        // can render themselves dimmed when they're
                        // the drag source. Peek (not take) keeps the
                        // payload alive across frames until release.
                        let dragged_source_id: Option<TabId> =
                            egui::DragAndDrop::payload::<TabDragPayload>(ui.ctx())
                                .map(|p| p.from_id);
                        let pointer_pos = ui.ctx().pointer_interact_pos();

                        // Drop target tracking — set by whichever chip
                        // the cursor sits over during an active drag.
                        let mut drop_target_idx: Option<usize> = None;

                        for (idx, tab) in col.tabs().iter().enumerate() {
                            let is_active = tab.id == col.active_id();
                            let title = tab.display_title();
                            let in_flight = tab.in_flight_ops.load(Ordering::SeqCst) > 0;
                            let pinned = tab.pinned.load(Ordering::SeqCst);
                            let is_drag_source = dragged_source_id == Some(tab.id);
                            let (chip_action, chip_response) = render_tab_chip(
                                ui,
                                theme,
                                tab.id,
                                &title,
                                is_active,
                                in_flight,
                                is_drag_source,
                                pinned,
                            );
                            if let Some(a) = chip_action {
                                action = Some(a);
                            }
                            if is_active && active_changed {
                                chip_response.scroll_to_me(Some(egui::Align::Center));
                            }
                            // Did this chip just start being dragged?
                            if chip_response.drag_started_by(egui::PointerButton::Primary) {
                                egui::DragAndDrop::set_payload(
                                    ui.ctx(),
                                    TabDragPayload {
                                        from_idx: idx,
                                        from_id: tab.id,
                                    },
                                );
                            }
                            // Is the cursor (during an active drag)
                            // over this chip's rect?
                            if dragged_source_id.is_some() {
                                if let Some(pos) = pointer_pos {
                                    if chip_response.rect.contains(pos) {
                                        drop_target_idx = Some(idx);
                                    }
                                }
                            }
                        }

                        // Pointer released this frame → commit the drop.
                        // `any_released` covers all buttons; we filter
                        // by checking that a drag payload exists.
                        let released = ui.input(|i| i.pointer.any_released());
                        if released {
                            if let Some(payload) =
                                egui::DragAndDrop::take_payload::<TabDragPayload>(ui.ctx())
                            {
                                if let Some(to_idx) = drop_target_idx {
                                    if payload.from_idx != to_idx {
                                        action = Some(TabBarAction::Reorder {
                                            from_idx: payload.from_idx,
                                            to_idx,
                                        });
                                    }
                                }
                            }
                        }
                    });
                });
                scroll_offset_x = scroll_out.state.offset.x;
                content_width = scroll_out.content_size.x;
                viewport_rect = scroll_out.inner_rect;
            });
        });
    });

    let info = TabScrollInfo {
        offset: scroll_offset_x,
        content_width,
        viewport_width: viewport_rect.width(),
    };

    // No overflow → nothing to scroll and no scrollbar strip will be
    // shown; drop any stale drag override so it can't keep forcing the
    // offset (strangling wheel scroll) if the tabs overflow again later.
    if !info.has_overflow() {
        ui.memory_mut(|m| m.data.remove_temp::<f32>(pill_offset_key()));
    }

    (action, info)
}

/// Render the draggable tab scrollbar into its own strip (the caller
/// puts this in a dedicated panel directly below the tab row, so it
/// never competes with the chips for vertical space). Always visible
/// while called — the caller only calls it when `info.has_overflow()`.
///
/// Click or drag anywhere on the track and the thumb centers under the
/// pointer, scrubbing the strip. The resulting offset is stashed in
/// `pill_offset_key()`; `render_tab_bar` applies it on the next frame
/// via `horizontal_scroll_offset`. The key is cleared the instant the
/// pointer releases, handing control back to wheel scroll /
/// scroll-into-view.
pub fn render_tab_scrollbar(ui: &mut egui::Ui, info: &TabScrollInfo, theme: &ThemeColors) {
    let content_w = info.content_width;
    let viewport_w = info.viewport_width;
    let key = pill_offset_key();

    // Track spans the full available width of the strip.
    let avail_w = ui.available_width();
    let (track_rect, response) = ui.allocate_exact_size(
        egui::vec2(avail_w, SCROLLBAR_THICKNESS),
        egui::Sense::click_and_drag(),
    );

    let thumb_fraction = (viewport_w / content_w).clamp(0.05, 1.0);
    let thumb_w = (track_rect.width() * thumb_fraction).max(24.0);
    let max_thumb_x = track_rect.width() - thumb_w;

    // Drag / click handling. `is_pointer_button_down_on` covers the
    // press (click-to-jump) and `dragged` the scrub; either way the
    // thumb centers on the pointer. Compute the live fraction so the
    // thumb tracks with no one-frame lag and persist the matching
    // scroll offset for the ScrollArea to pick up next frame.
    let interacting = response.is_pointer_button_down_on() || response.dragged();
    let position_fraction = if interacting && max_thumb_x > 0.5 {
        if let Some(px) = response.interact_pointer_pos().map(|p| p.x) {
            let frac = pill_fraction_from_pointer(px, track_rect.left(), thumb_w, max_thumb_x);
            ui.memory_mut(|m| m.data.insert_temp(key, frac * (content_w - viewport_w)));
            frac
        } else {
            natural_position_fraction(info.offset, content_w, viewport_w)
        }
    } else {
        // Released / idle — drop the override so wheel scroll resumes.
        ui.memory_mut(|m| m.data.remove_temp::<f32>(key));
        natural_position_fraction(info.offset, content_w, viewport_w)
    };

    // PointingHand (matches the chips / buttons), not Grab/Grabbing —
    // winit has no native grab cursor on Windows and falls back to
    // SizeAll (the 4-way move arrow), which reads wrong for a scrollbar.
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    painter.rect_filled(
        track_rect,
        egui::CornerRadius::same(3),
        theme.outline_variant,
    );

    let thumb_x = track_rect.left() + max_thumb_x * position_fraction;
    let thumb_rect = egui::Rect::from_min_size(
        egui::pos2(thumb_x, track_rect.top()),
        egui::vec2(thumb_w, SCROLLBAR_THICKNESS),
    );
    // Brighten the thumb while grabbed for tactile feedback.
    let thumb_color = if interacting {
        theme.primary
    } else {
        theme.outline
    };
    painter.rect_filled(thumb_rect, egui::CornerRadius::same(3), thumb_color);
}

/// Thumb position (0..=1) for a given scroll offset — the resting
/// position when the scrollbar isn't being dragged.
fn natural_position_fraction(offset: f32, content_w: f32, viewport_w: f32) -> f32 {
    if content_w > viewport_w {
        (offset / (content_w - viewport_w)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Map a pointer x-coordinate to a thumb position fraction (0..=1) such
/// that the thumb *centers* under the pointer, clamped to the track.
/// Factored out of `render_tab_scrollbar` so the clamp edges are unit-
/// testable without an egui pointer harness. Caller guarantees
/// `max_thumb_x > 0`.
fn pill_fraction_from_pointer(
    pointer_x: f32,
    track_left: f32,
    thumb_w: f32,
    max_thumb_x: f32,
) -> f32 {
    let thumb_left = (pointer_x - thumb_w / 2.0).clamp(track_left, track_left + max_thumb_x);
    (thumb_left - track_left) / max_thumb_x
}

#[cfg(test)]
mod pill_tests {
    use super::*;

    #[test]
    fn pointer_at_track_start_yields_zero() {
        // Pointer at (or before) the left edge → thumb pinned left.
        assert_eq!(pill_fraction_from_pointer(100.0, 100.0, 40.0, 200.0), 0.0);
        assert_eq!(pill_fraction_from_pointer(0.0, 100.0, 40.0, 200.0), 0.0);
    }

    #[test]
    fn pointer_past_track_end_yields_one() {
        // Pointer beyond the right travel limit → thumb pinned right.
        // track_left=100, max_thumb_x=200 → thumb_left clamps to 300.
        assert_eq!(
            pill_fraction_from_pointer(10_000.0, 100.0, 40.0, 200.0),
            1.0
        );
    }

    #[test]
    fn pointer_centers_thumb_under_cursor() {
        // thumb_w=40 so half=20. Pointer at 220 → thumb_left=200 →
        // (200-100)/200 = 0.5.
        let frac = pill_fraction_from_pointer(220.0, 100.0, 40.0, 200.0);
        assert!((frac - 0.5).abs() < 1e-6, "frac was {frac}");
    }

    #[test]
    fn natural_fraction_handles_no_overflow() {
        // content fits in viewport → always pinned left, no divide-by-zero.
        assert_eq!(natural_position_fraction(0.0, 100.0, 100.0), 0.0);
        assert_eq!(natural_position_fraction(50.0, 80.0, 120.0), 0.0);
    }

    #[test]
    fn natural_fraction_maps_offset_to_position() {
        // content 300, viewport 100 → max offset 200. offset 100 → 0.5.
        assert!((natural_position_fraction(100.0, 300.0, 100.0) - 0.5).abs() < 1e-6);
        // Over-scroll clamps to 1.0.
        assert_eq!(natural_position_fraction(9999.0, 300.0, 100.0), 1.0);
    }
}

fn scale_alpha(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let a = (color.a() as f32 * alpha.clamp(0.0, 1.0)) as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

// `is_drag_source` = true if this chip is currently the source of an
// in-flight drag. Renders with reduced opacity so the user can tell
// which tab they grabbed when the cursor moves over another chip.
// `pinned` toggles a leading pin glyph and the Pin/Unpin context-menu
// label.
fn render_tab_chip(
    ui: &mut egui::Ui,
    theme: &ThemeColors,
    id: TabId,
    title: &str,
    is_active: bool,
    in_flight: bool,
    is_drag_source: bool,
    pinned: bool,
) -> (Option<TabBarAction>, egui::Response) {
    let font_id = egui::FontId::proportional(FONT_SIZE);
    let text_color = if is_active {
        theme.on_primary_container
    } else {
        theme.on_surface_variant
    };

    // Pin glyph (when pinned) precedes the in-flight glyph (when busy)
    // which precedes the title. Format: `[📌] [○] title`. Both glyphs
    // are inline so the truncation cache treats label changes
    // (pin/unpin, in-flight start/stop) as cache invalidations.
    let raw_label = match (pinned, in_flight) {
        (true, true) => format!(
            "{} {} {}",
            egui_phosphor::regular::PUSH_PIN,
            egui_phosphor::regular::CIRCLE,
            title
        ),
        (true, false) => {
            format!("{} {}", egui_phosphor::regular::PUSH_PIN, title)
        }
        (false, true) => {
            format!("{} {}", egui_phosphor::regular::CIRCLE, title)
        }
        (false, false) => title.to_string(),
    };

    // Active tab gets a wider title cap (Chrome-style adaptive width).
    let max_title_w = if is_active {
        TAB_TITLE_MAX_WIDTH_ACTIVE
    } else {
        TAB_TITLE_MAX_WIDTH_INACTIVE
    };
    // Cache the truncation result so we don't rerun the binary-search
    // layout probes every frame while the cursor moves over the panel.
    // Key by (label, max_width_int): when either the title changes
    // (e.g. tab switches active state with different max_w, or archive
    // path changes) we recompute; otherwise we reuse.
    let cache_key = egui::Id::new(("tab_bar/truncate", id, max_title_w as u32));
    let cached: Option<(String, bool, String)> = ui.memory(|m| m.data.get_temp(cache_key));
    let (label_text, was_truncated) = match cached {
        // Cache hit and the source label is unchanged.
        Some((cached_src, was_trunc, cached_label)) if cached_src == raw_label => {
            (cached_label, was_trunc)
        }
        _ => {
            let computed =
                truncate_to_width(ui.painter(), &raw_label, &font_id, text_color, max_title_w);
            ui.memory_mut(|m| {
                m.data.insert_temp(
                    cache_key,
                    (raw_label.clone(), computed.1, computed.0.clone()),
                )
            });
            computed
        }
    };

    let probe = ui
        .painter()
        .layout_no_wrap(label_text.clone(), font_id.clone(), text_color);
    let chip_width = (probe.size().x + TAB_H_PAD * 2.0 + TAB_CLOSE_GAP + TAB_CLOSE_HIT_SIZE).ceil();
    let chip_size = egui::vec2(chip_width, TAB_HEIGHT);
    // Sense both click and drag — clicks dispatch Switch/Close, drags
    // reorder. egui's built-in drag threshold keeps a stationary click
    // from being misread as a drag.
    let (rect, response) = ui.allocate_exact_size(chip_size, egui::Sense::click_and_drag());

    let (bg_fill, stroke_color) = if is_active {
        (theme.primary_container, theme.primary)
    } else if response.hovered() {
        (theme.surface_variant, theme.outline)
    } else {
        (theme.surface, theme.outline_variant)
    };
    // Drag-source dim: 45% opacity on the fill so the user can see
    // which chip they grabbed even when the cursor moves over another.
    let drag_alpha = if is_drag_source { 0.45 } else { 1.0 };
    let bg_fill = scale_alpha(bg_fill, drag_alpha);
    let stroke_color = scale_alpha(stroke_color, drag_alpha);
    let chip_text_color = scale_alpha(text_color, drag_alpha);

    let painter = ui.painter();
    painter.rect(
        rect,
        egui::CornerRadius::same(TAB_CORNER_RADIUS),
        bg_fill,
        egui::Stroke::new(1.0_f32, stroke_color),
        egui::StrokeKind::Middle,
    );

    paint_text_left_in_rect_visually_centered(
        painter,
        label_text,
        font_id.clone(),
        chip_text_color,
        rect,
        TAB_H_PAD,
    );

    let close_center = egui::pos2(
        rect.right() - TAB_H_PAD - TAB_CLOSE_HIT_SIZE / 2.0,
        rect.center().y,
    );
    let close_rect = egui::Rect::from_center_size(
        close_center,
        egui::vec2(TAB_CLOSE_HIT_SIZE, TAB_CLOSE_HIT_SIZE),
    );
    paint_text_visually_centered(
        painter,
        egui_phosphor::regular::X,
        font_id,
        chip_text_color,
        close_center,
    );

    // Switch cursor to the grabbing hand while a drag is in flight on
    // this chip; otherwise the regular pointing hand on hover.
    let response = if response.dragged() {
        response.on_hover_cursor(egui::CursorIcon::Grabbing)
    } else {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    // Full title on hover when truncated. Always-on tooltip with the
    // archive's full filename is also useful for quickly distinguishing
    // tabs whose visible prefixes look similar.
    let response = if was_truncated {
        response.on_hover_text(title)
    } else {
        response
    };

    let mut action = if response.clicked() {
        response.interact_pointer_pos().map(|click_pos| {
            if close_rect.contains(click_pos) {
                TabBarAction::Close(id)
            } else {
                TabBarAction::Switch(id)
            }
        })
    } else if response.clicked_by(egui::PointerButton::Middle) {
        // Middle-click anywhere on the chip closes it — matches the
        // standard browser tab convention. The close hit-rect handles
        // primary-click closes; middle-click is the bigger-target
        // alternative for users who don't want to aim at the X.
        Some(TabBarAction::Close(id))
    } else {
        None
    };
    // Right-click context menu — matches browser tab conventions.
    // egui takes care of dismissal on click-outside and on entry click.
    response.context_menu(|ui| {
        let pin_label = if pinned { "Unpin tab" } else { "Pin tab" };
        if ui.button(pin_label).clicked() {
            action = Some(TabBarAction::SetPinned(id, !pinned));
            ui.close();
        }
        ui.separator();
        if ui.button("Close tab").clicked() {
            action = Some(TabBarAction::Close(id));
            ui.close();
        }
        if ui.button("Close other tabs").clicked() {
            action = Some(TabBarAction::CloseOthers(id));
            ui.close();
        }
        if ui.button("Close tabs to the right").clicked() {
            action = Some(TabBarAction::CloseToRight(id));
            ui.close();
        }
        ui.separator();
        if ui.button("Duplicate tab").clicked() {
            action = Some(TabBarAction::Duplicate(id));
            ui.close();
        }
    });
    (action, response)
}

/// Truncate `text` with a trailing `…` so its painted width is at most
/// `max_width` pixels. Returns the (possibly truncated) string and a
/// bool indicating whether truncation occurred. Binary-searches the
/// longest fitting prefix; O(log n) layout calls.
fn truncate_to_width(
    painter: &egui::Painter,
    text: &str,
    font_id: &egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> (String, bool) {
    let full_probe = painter.layout_no_wrap(text.to_string(), font_id.clone(), color);
    if full_probe.size().x <= max_width {
        return (text.to_string(), false);
    }
    let chars: Vec<char> = text.chars().collect();
    // Binary search the largest prefix length such that `prefix + "…"`
    // still fits.
    let mut lo: usize = 0;
    let mut hi: usize = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
        let cand_probe = painter.layout_no_wrap(candidate, font_id.clone(), color);
        if cand_probe.size().x <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let truncated: String = if lo == 0 {
        "\u{2026}".to_string()
    } else {
        chars[..lo].iter().collect::<String>() + "\u{2026}"
    };
    (truncated, true)
}

/// Chevron-down button that opens the overflow popup listing every
/// open tab. Visually mirrors `render_plus_button` so the two sit
/// flush against each other at the right edge of the strip.
fn render_chevron_button(ui: &mut egui::Ui, theme: &ThemeColors) -> egui::Response {
    let font_id = egui::FontId::proportional(FONT_SIZE);
    let icon = egui_phosphor::regular::CARET_DOWN;
    let probe =
        ui.painter()
            .layout_no_wrap(icon.to_string(), font_id.clone(), theme.on_surface_variant);
    let size = egui::vec2((probe.size().x + TAB_H_PAD * 2.0).ceil(), TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let (bg_fill, stroke_color) = if response.hovered() {
        (theme.surface_variant, theme.outline)
    } else {
        (theme.surface, theme.outline_variant)
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        egui::CornerRadius::same(TAB_CORNER_RADIUS),
        bg_fill,
        egui::Stroke::new(1.0_f32, stroke_color),
        egui::StrokeKind::Middle,
    );
    paint_text_visually_centered(
        painter,
        icon,
        font_id,
        theme.on_surface_variant,
        rect.center(),
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("List all tabs")
}

fn render_plus_button(ui: &mut egui::Ui, theme: &ThemeColors) -> egui::Response {
    let font_id = egui::FontId::proportional(FONT_SIZE);
    let icon = egui_phosphor::regular::PLUS;
    let probe =
        ui.painter()
            .layout_no_wrap(icon.to_string(), font_id.clone(), theme.on_surface_variant);
    let size = egui::vec2((probe.size().x + TAB_H_PAD * 2.0).ceil(), TAB_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let (bg_fill, stroke_color) = if response.hovered() {
        (theme.surface_variant, theme.outline)
    } else {
        (theme.surface, theme.outline_variant)
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        egui::CornerRadius::same(TAB_CORNER_RADIUS),
        bg_fill,
        egui::Stroke::new(1.0_f32, stroke_color),
        egui::StrokeKind::Middle,
    );
    paint_text_visually_centered(
        painter,
        icon,
        font_id,
        theme.on_surface_variant,
        rect.center(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
