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
//! | [tab1] [tab2 active] [tab3] ...   | [+]      |
//! | ----------------                              |  <- position pill
//! +-----------------------------------------------+
//! ```
//!
//! The `+` button is pinned to the right edge outside the scroll area
//! so it stays reachable regardless of how many tabs you have open.
//! Vertical mouse-wheel over the tab strip scrolls horizontally.
//! Switching the active tab (via click or keyboard) scrolls it into
//! view if it was off-screen.
//!
//! When tabs overflow horizontally, a position pill renders below the
//! strip showing `offset / max_offset` as a colored thumb on a track.
//! Invisible when content fits.

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
    Reorder { from_idx: usize, to_idx: usize },
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
/// When true, the position pill always renders when overflow exists
/// (bypasses the hover-/scroll-driven visibility). Useful for verifying
/// pill placement / size during development.
const DEBUG_ALWAYS_SHOW_PILL: bool = false;
/// Corner radius for tab chips and the `+` button. Currently 0 to match
/// arclain's brutalist visual language; once a theme system exposes a
/// `tab_corner_radius` variable, this becomes theme-driven.
const TAB_CORNER_RADIUS: u8 = 0;

/// Height of the position pill track (the thin colored bar below the
/// tab strip indicating scroll position).
const POSITION_PILL_HEIGHT: f32 = 5.0;
const POSITION_PILL_GAP: f32 = 2.0;

/// Render the multi-archive tab bar. Returns Some(action) when the user
/// clicked something; the caller applies the action to its
/// `TabsCollection`.
pub fn render_tab_bar(
    ui: &mut egui::Ui,
    col: &TabsCollection,
    theme: &ThemeColors,
) -> Option<TabBarAction> {
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

    ui.vertical(|ui| {
        // Zero vertical spacing between the row and the position pill —
        // we control the gap via POSITION_PILL_GAP explicitly. Without
        // this, ui.vertical's default item_spacing.y (~4px) eats into
        // the panel's fixed height and clips the pill out.
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
                            if ui
                                .selectable_label(is_active, &title)
                                .clicked()
                            {
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
                let scroll_out = egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
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
                                if i.smooth_scroll_delta.x == 0.0
                                    && i.smooth_scroll_delta.y != 0.0
                                {
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
                                let in_flight =
                                    tab.in_flight_ops.load(Ordering::SeqCst) > 0;
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
                                        TabDragPayload { from_idx: idx, from_id: tab.id },
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

        // Compute overflow + paint position pill (gated by hover /
        // recent scroll / debug).
        let viewport_w = viewport_rect.width();
        let max_offset = (content_width - viewport_w).max(0.0);
        let has_overflow = max_offset > 0.5;

        if has_overflow {
            // Track previous scroll offset across frames to detect
            // "actively scrolling". When the offset changed since last
            // frame, refresh `last_scroll_time` to the current time.
            let prev_offset_key = egui::Id::new("tab_bar/prev_offset_x");
            let prev_offset: f32 = ui
                .memory(|m| m.data.get_temp(prev_offset_key))
                .unwrap_or(0.0);
            let scrolling_now = (scroll_offset_x - prev_offset).abs() > 0.5;
            ui.memory_mut(|m| m.data.insert_temp(prev_offset_key, scroll_offset_x));

            let last_scroll_time_key = egui::Id::new("tab_bar/last_scroll_time");
            let now = ui.input(|i| i.time);
            if scrolling_now {
                ui.memory_mut(|m| m.data.insert_temp(last_scroll_time_key, now));
            }
            let last_scroll_time: f64 = ui
                .memory(|m| m.data.get_temp(last_scroll_time_key))
                .unwrap_or(0.0);
            let scrolled_recently = (now - last_scroll_time) < 0.6; // 600ms after-scroll lingering

            // Hover anywhere over the tab-bar panel keeps the pill visible.
            let panel_hovered = ui.rect_contains_pointer(viewport_rect)
                || ui
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| {
                        // Slightly expand the test rect upward so hovering
                        // the chip strip (which sits above the viewport's
                        // pill area) also counts.
                        let mut r = viewport_rect;
                        r.min.y -= TAB_HEIGHT;
                        r.contains(p)
                    })
                    .unwrap_or(false);

            let target_visible = panel_hovered || scrolled_recently || DEBUG_ALWAYS_SHOW_PILL;
            // Fade the pill in/out over 150ms.
            let alpha = ui.ctx().animate_bool_with_time(
                egui::Id::new("tab_bar/pill_alpha"),
                target_visible,
                0.15,
            );

            // Skip rendering entirely when fully transparent — avoids
            // paying for the layout allocation when hidden.
            if alpha > 0.01 {
                paint_position_pill(
                    ui,
                    viewport_rect,
                    scroll_offset_x,
                    content_width,
                    viewport_w,
                    theme,
                    alpha,
                );
            } else {
                // Still allocate the vertical space so the panel height
                // stays consistent and the pill area doesn't visually
                // jump.
                let _ = ui.allocate_exact_size(
                    egui::vec2(viewport_w, POSITION_PILL_HEIGHT + POSITION_PILL_GAP),
                    egui::Sense::hover(),
                );
            }
        }
    });

    action
}

/// Paint a thin position pill below the scroll viewport. Allocates
/// `POSITION_PILL_HEIGHT + POSITION_PILL_GAP` vertical pixels in `ui`.
/// `alpha` multiplies both the track and thumb colors (0..=1), enabling
/// the hover-/scroll-driven fade-in/out.
fn paint_position_pill(
    ui: &mut egui::Ui,
    viewport: egui::Rect,
    offset: f32,
    content_w: f32,
    viewport_w: f32,
    theme: &ThemeColors,
    alpha: f32,
) {
    let total_h = POSITION_PILL_HEIGHT + POSITION_PILL_GAP;
    let (track_rect, _) = ui.allocate_exact_size(
        egui::vec2(viewport.width(), total_h),
        egui::Sense::hover(),
    );
    let track = egui::Rect::from_min_size(
        egui::pos2(viewport.left(), track_rect.top() + POSITION_PILL_GAP),
        egui::vec2(viewport.width(), POSITION_PILL_HEIGHT),
    );

    let painter = ui.painter();
    painter.rect_filled(
        track,
        egui::CornerRadius::same(2),
        scale_alpha(theme.outline_variant, alpha),
    );

    let thumb_fraction = (viewport_w / content_w).clamp(0.05, 1.0);
    let thumb_w = (track.width() * thumb_fraction).max(20.0);
    let max_thumb_x = track.width() - thumb_w;
    let position_fraction = if content_w > viewport_w {
        (offset / (content_w - viewport_w)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let thumb_x = track.left() + max_thumb_x * position_fraction;
    let thumb_rect = egui::Rect::from_min_size(
        egui::pos2(thumb_x, track.top()),
        egui::vec2(thumb_w, POSITION_PILL_HEIGHT),
    );
    painter.rect_filled(
        thumb_rect,
        egui::CornerRadius::same(2),
        scale_alpha(theme.primary, alpha),
    );
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
    let cached: Option<(String, bool, String)> =
        ui.memory(|m| m.data.get_temp(cache_key));
    let (label_text, was_truncated) = match cached {
        // Cache hit and the source label is unchanged.
        Some((cached_src, was_trunc, cached_label)) if cached_src == raw_label => {
            (cached_label, was_trunc)
        }
        _ => {
            let computed = truncate_to_width(
                ui.painter(),
                &raw_label,
                &font_id,
                text_color,
                max_title_w,
            );
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
    let chip_width =
        (probe.size().x + TAB_H_PAD * 2.0 + TAB_CLOSE_GAP + TAB_CLOSE_HIT_SIZE).ceil();
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
        egui::Stroke::new(1.0, stroke_color),
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
        response
            .interact_pointer_pos()
            .map(|click_pos| {
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
        let cand_probe =
            painter.layout_no_wrap(candidate, font_id.clone(), color);
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
    let probe = ui.painter().layout_no_wrap(
        icon.to_string(),
        font_id.clone(),
        theme.on_surface_variant,
    );
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
        egui::Stroke::new(1.0, stroke_color),
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
    let probe = ui.painter().layout_no_wrap(
        icon.to_string(),
        font_id.clone(),
        theme.on_surface_variant,
    );
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
        egui::Stroke::new(1.0, stroke_color),
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
