//! Text widget with pixel-aligned rendering and automatic theme support
//!
//! This widget provides consistent text rendering with:
//! - Automatic theme colors (retrieved from egui context)
//! - Pixel alignment for custom painting
//!
//! Theme colors are stored in egui's context via `set_theme()` and automatically
//! retrieved by Text widgets, similar to Flutter's Theme.of(context).

use arclain_theme::ThemeColors;
use egui::{Align2, Color32, FontFamily, FontId, Id, Pos2, Rect, Response, Ui, Widget};

/// ID for storing theme colors in egui's context data
const THEME_COLORS_ID: &str = "arclain_theme_colors";

/// Rounds a position to the nearest pixel boundary.
#[inline]
pub fn pixel_align(pos: Pos2) -> Pos2 {
    egui::pos2(pos.x.round(), pos.y.round())
}

/// Store theme colors in egui's context for automatic retrieval by widgets.
/// Call this once during app initialization or when theme changes.
///
/// # Example
/// ```ignore
/// // In your app's update function
/// arclain_widgets::set_theme(ctx, theme.colors.clone());
/// ```
pub fn set_theme(ctx: &egui::Context, colors: ThemeColors) {
    ctx.data_mut(|data| {
        data.insert_temp(Id::new(THEME_COLORS_ID), colors);
    });
}

/// Get theme colors from egui's context.
/// Returns None if set_theme() hasn't been called.
pub fn get_theme(ctx: &egui::Context) -> Option<ThemeColors> {
    ctx.data(|data| data.get_temp::<ThemeColors>(Id::new(THEME_COLORS_ID)))
}

/// A text widget with automatic theme colors and pixel-aligned custom painting.
///
/// Theme colors are automatically retrieved from egui's context (set via `set_theme()`).
///
/// # Example
/// ```ignore
/// // Basic usage - automatically uses theme's on_surface color
/// Text::new("Hello World").show(ui);
///
/// // Muted/secondary text - uses on_surface_variant
/// Text::new("Subtitle").muted().show(ui);
///
/// // With customization
/// Text::new("Title")
///     .size(18.0)
///     .strong()
///     .show(ui);
///
/// // Override color explicitly
/// Text::new("Warning").color(Color32::RED).show(ui);
///
/// // For custom painting (pixel-aligned)
/// Text::new("Custom")
///     .size(14.0)
///     .draw(painter, ctx, pos, Align2::LEFT_CENTER);
/// ```
pub struct Text<'a> {
    text: &'a str,
    size: f32,
    color_override: Option<Color32>,
    muted: bool,
    strong: bool,
    family: FontFamily,
}

impl<'a> Text<'a> {
    /// Create a new text widget with the given content.
    /// Color defaults to theme's `on_surface` color.
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            size: 14.0,
            color_override: None,
            muted: false,
            strong: false,
            family: FontFamily::Proportional,
        }
    }

    /// Set the font size in points.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Set an explicit color (overrides theme).
    pub fn color(mut self, color: Color32) -> Self {
        self.color_override = Some(color);
        self
    }

    /// Use muted/secondary color (on_surface_variant).
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    /// Make the text bold/strong.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    /// Use monospace font.
    pub fn monospace(mut self) -> Self {
        self.family = FontFamily::Monospace;
        self
    }

    /// Get the resolved color from context or fallback.
    fn resolved_color(&self, ui: &Ui) -> Color32 {
        if let Some(color) = self.color_override {
            return color;
        }

        // Try to get theme from context
        if let Some(colors) = get_theme(ui.ctx()) {
            if self.muted {
                colors.on_surface_variant
            } else {
                colors.on_surface
            }
        } else {
            // Fallback to UI visuals if no theme set
            if self.muted {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            }
        }
    }

    /// Get the resolved color for painting (with context).
    fn resolved_paint_color(&self, ctx: &egui::Context) -> Color32 {
        if let Some(color) = self.color_override {
            return color;
        }

        if let Some(colors) = get_theme(ctx) {
            if self.muted {
                colors.on_surface_variant
            } else {
                colors.on_surface
            }
        } else {
            // Fallback - shouldn't happen if set_theme is called
            Color32::WHITE
        }
    }

    /// Get the font ID for this text.
    fn font_id(&self) -> FontId {
        FontId::new(self.size, self.family.clone())
    }

    /// Build a RichText from this configuration.
    pub fn to_rich_text(&self, ui: &Ui) -> egui::RichText {
        let mut rt = egui::RichText::new(self.text)
            .size(self.size)
            .color(self.resolved_color(ui))
            .family(self.family.clone());
        if self.strong {
            rt = rt.strong();
        }
        rt
    }

    /// Show as a label widget (standard egui Label).
    pub fn show(self, ui: &mut Ui) -> Response {
        ui.label(self.to_rich_text(ui))
    }

    /// Draw directly to the painter at a pixel-aligned position.
    ///
    /// Use this for custom painting where you need precise positioning.
    pub fn draw(self, painter: &egui::Painter, ctx: &egui::Context, pos: Pos2, align: Align2) {
        painter.text(
            pixel_align(pos),
            align,
            self.text,
            self.font_id(),
            self.resolved_paint_color(ctx),
        );
    }

    /// Draw centered within a rectangle at pixel-aligned position.
    pub fn draw_in_rect(
        self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        rect: Rect,
        align: Align2,
    ) {
        let pos = align_position_in_rect(rect, align);
        painter.text(
            pos,
            align,
            self.text,
            self.font_id(),
            self.resolved_paint_color(ctx),
        );
    }
}

impl<'a> Widget for Text<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

/// Calculates a pixel-aligned position for the given alignment within a rect.
fn align_position_in_rect(rect: Rect, align: Align2) -> Pos2 {
    let x = match align.x() {
        egui::Align::Min => rect.min.x,
        egui::Align::Center => rect.center().x,
        egui::Align::Max => rect.max.x,
    };

    let y = match align.y() {
        egui::Align::Min => rect.min.y,
        egui::Align::Center => rect.center().y,
        egui::Align::Max => rect.max.y,
    };

    pixel_align(egui::pos2(x, y))
}
