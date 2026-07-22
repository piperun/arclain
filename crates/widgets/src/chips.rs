//! Chip widget - a pill-shaped label/badge

use arclain_theme::ThemeColors;
use egui::{Response, Ui, Widget};

/// A pill-shaped chip/badge label.
///
/// Defaults to a passive label (hover-only response). Call
/// `.clickable(true)` to opt into click semantics — the returned
/// `Response` will then react to `clicked()` and the cursor will
/// switch to a hand pointer over the chip.
///
/// Use `.icon(...)` to prefix a phosphor (or other) icon glyph to
/// the text. The icon is rendered as a separate label inside an
/// `ui.horizontal_centered` so it shares vertical center with the
/// text — combining icon + text into one `RichText` doesn't work
/// because the phosphor font has a y_offset_factor tweak applied at
/// load time (see `arclain_theme::fonts`), so the icon glyph baseline
/// sits higher than the regular text baseline.
pub struct Chips<'a> {
    text: &'a str,
    icon: Option<&'a str>,
    colors: Option<&'a ThemeColors>,
    stroke_color: Option<egui::Color32>,
    background_color: Option<egui::Color32>,
    text_color: Option<egui::Color32>,
    clickable: bool,
    debug_lines: bool,
}

impl<'a> Chips<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            icon: None,
            colors: None,
            stroke_color: None,
            background_color: None,
            text_color: None,
            clickable: false,
            debug_lines: false,
        }
    }

    /// Draw colored guide lines so you can SEE what's happening with
    /// vertical alignment. Magenta = pill geometric center; cyan =
    /// galley bounding-box center; yellow = visible-glyph top
    /// (galley top + ascent); green = baseline. Use to tune
    /// inner_margin without guessing.
    pub fn debug_lines(mut self, on: bool) -> Self {
        self.debug_lines = on;
        self
    }

    /// Set an icon glyph to render before the text. Pass an
    /// `egui_phosphor` constant (e.g. `egui_phosphor::regular::CHECK_CIRCLE`)
    /// or any printable string. Rendered as a separate label inside
    /// the chip so vertical centering against the text works
    /// regardless of the icon font's baseline tweak.
    pub fn icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_theme_colors(mut self, colors: &'a ThemeColors) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Override the stroke/border color
    pub fn stroke_color(mut self, color: egui::Color32) -> Self {
        self.stroke_color = Some(color);
        self
    }

    /// Override the background color
    pub fn background_color(mut self, color: egui::Color32) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Override the text color (defaults to `on_surface` from the
    /// theme or the inactive widget fg color).
    pub fn text_color(mut self, color: egui::Color32) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Make the chip clickable. The returned `Response` will report
    /// `clicked()`, hover state, and a hand-pointer cursor. Default
    /// is `false` (chip is a passive label).
    pub fn clickable(mut self, clickable: bool) -> Self {
        self.clickable = clickable;
        self
    }
}

impl<'a> Widget for Chips<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (bg_fill, stroke, text_color) = if let Some(colors) = self.colors {
            let stroke_col = self.stroke_color.unwrap_or(colors.outline);
            let bg = self.background_color.unwrap_or(colors.surface_variant);
            let txt = self.text_color.unwrap_or(colors.on_surface);
            (bg, egui::Stroke::new(1.0_f32, stroke_col), txt)
        } else {
            let bg = self
                .background_color
                .unwrap_or(ui.visuals().widgets.inactive.bg_fill);
            let txt = self
                .text_color
                .unwrap_or(ui.visuals().widgets.inactive.fg_stroke.color);
            (
                bg,
                egui::Stroke::new(1.0_f32, ui.visuals().widgets.inactive.bg_stroke.color),
                txt,
            )
        };

        // Manual paint via the standardized text_layout helpers, so
        // every widget that needs visually-centered text in arclain
        // shares the same correct behavior. See `text_layout.rs` for
        // why mesh_bounds-based centering is the right answer rather
        // than relying on egui's bounding-box midpoint anchors.
        let combined = if let Some(icon) = self.icon {
            format!("{} {}", icon, self.text)
        } else {
            self.text.to_string()
        };

        let font_id = egui::FontId::proportional(12.0);
        let h_pad = 10.0_f32;
        let chip_height = 24.0_f32;

        // Pre-layout to size the chip rect.
        let probe_galley =
            ui.painter()
                .layout_no_wrap(combined.clone(), font_id.clone(), text_color);
        let chip_size = egui::vec2((probe_galley.size().x + h_pad * 2.0).ceil(), chip_height);
        let (rect, response) = ui.allocate_exact_size(
            chip_size,
            if self.clickable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );

        ui.painter().rect(
            rect,
            egui::CornerRadius::same((chip_height / 2.0) as u8),
            bg_fill,
            stroke,
            egui::StrokeKind::Middle,
        );

        let painted_rect = crate::text_layout::paint_text_left_in_rect_visually_centered(
            ui.painter(),
            combined,
            font_id,
            text_color,
            rect,
            h_pad,
        );

        // Standardized debug overlay (widgets::paint_centering_debug) —
        // shows pill rect, painted-text rect, and the (dx, dy) offset
        // between their centers. Lit by the per-chip `debug_lines`
        // builder OR the project-wide EGUI_UI_DEBUG_GUIDELINES env
        // var (see `widgets::debug::ui_debug_guidelines_enabled`).
        // Debug-only — stripped in release so neither the overlay
        // nor `painted_rect`'s consumer stays in the release binary.
        #[cfg(debug_assertions)]
        crate::debug::paint_centering_debug(
            ui.painter(),
            rect,
            painted_rect,
            "chip",
            self.debug_lines || crate::debug::ui_debug_guidelines_enabled(),
        );
        #[cfg(not(debug_assertions))]
        let _ = painted_rect;

        if self.clickable {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            response
        }
    }
}
