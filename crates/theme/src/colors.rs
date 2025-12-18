//! Theme colors implementing Flutter ColorScheme pattern
//!
//! Provides semantic color naming with core colors (primary, secondary, tertiary, error)
//! each having `on_*` and `*_container` variants, plus surface and status colors.

use egui::Color32;

/// Theme colors implementing Flutter ColorScheme pattern
///
/// # Color Hierarchy
/// - **Tier 1 (Core)**: primary, secondary, tertiary, error - each with on_* and *_container variants
/// - **Tier 2 (Surface)**: surface, on_surface, surface_variant, outline
/// - **Tier 3 (Status)**: warning, success, info states
#[derive(Clone, Debug)]
pub struct ThemeColors {
    // =========================================================================
    // TIER 1: CORE SEMANTIC COLORS
    // =========================================================================

    // --- Primary ---
    /// The primary color used for major UI elements (buttons, active states)
    pub primary: Color32,
    /// Content color drawn on top of the primary color
    pub on_primary: Color32,
    /// A lighter/softer container using the primary color
    pub primary_container: Color32,
    /// Content color drawn on top of the primary container
    pub on_primary_container: Color32,

    // --- Secondary ---
    /// Secondary color for less prominent elements
    pub secondary: Color32,
    /// Content color drawn on top of the secondary color
    pub on_secondary: Color32,
    /// A lighter/softer container using the secondary color
    pub secondary_container: Color32,
    /// Content color drawn on top of the secondary container
    pub on_secondary_container: Color32,

    // --- Tertiary ---
    /// Tertiary accent color for additional emphasis
    pub tertiary: Color32,
    /// Content color drawn on top of the tertiary color
    pub on_tertiary: Color32,
    /// A lighter/softer container using the tertiary color
    pub tertiary_container: Color32,
    /// Content color drawn on top of the tertiary container
    pub on_tertiary_container: Color32,

    // --- Error ---
    /// Error color for destructive actions and error states
    pub error: Color32,
    /// Content color drawn on top of the error color
    pub on_error: Color32,
    /// A lighter/softer container for error states
    pub error_container: Color32,
    /// Content color drawn on top of the error container
    pub on_error_container: Color32,

    // =========================================================================
    // TIER 2: SURFACE COLORS
    // =========================================================================
    /// The background color of the main window/surface
    pub surface: Color32,
    /// Content color drawn on top of the surface
    pub on_surface: Color32,
    /// A variant of the surface color (e.g. for panels or cards)
    pub surface_variant: Color32,
    /// Content color drawn on top of the surface variant
    pub on_surface_variant: Color32,
    /// Color for borders and dividers
    pub outline: Color32,
    /// A subtle variant of outline for less prominent borders
    pub outline_variant: Color32,

    // =========================================================================
    // TIER 3: STATUS COLORS (for notifications, badges, etc.)
    // =========================================================================
    /// Warning color for cautionary states
    pub warning: Color32,
    /// Content color drawn on top of warning
    pub on_warning: Color32,
    /// Success color for positive states
    pub success: Color32,
    /// Content color drawn on top of success
    pub on_success: Color32,
    /// Info color for informational states
    pub info: Color32,
    /// Content color drawn on top of info
    pub on_info: Color32,

    // =========================================================================
    // SELECTION & INTERACTION
    // =========================================================================
    /// Selection/highlight color (e.g., for selected list items)
    pub selection: Color32,
    /// Inverse surface for things like snackbars, tooltips
    pub inverse_surface: Color32,
    /// Content on inverse surface
    pub inverse_on_surface: Color32,
    /// Scrim/overlay color for modals
    pub scrim: Color32,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self::light()
    }
}

impl ThemeColors {
    // =========================================================================
    // BUILDER: Create from 4 base colors
    // =========================================================================

    /// Create a theme from just 4 base colors, auto-deriving the rest.
    ///
    /// This follows Flutter's pattern where you can define minimal colors
    /// and the rest are computed automatically.
    pub fn from_seed(
        primary: Color32,
        secondary: Color32,
        tertiary: Color32,
        surface: Color32,
        is_dark: bool,
    ) -> Self {
        let error = Color32::from_rgb(186, 26, 26);

        // Auto-derive on_* colors based on luminance
        let on_primary = contrast_color(primary);
        let on_secondary = contrast_color(secondary);
        let on_tertiary = contrast_color(tertiary);
        let on_surface = contrast_color(surface);
        let on_error = Color32::WHITE;

        // Derive container colors (lighter/softer versions)
        let primary_container = soften_color(primary, is_dark);
        let secondary_container = soften_color(secondary, is_dark);
        let tertiary_container = soften_color(tertiary, is_dark);
        let error_container = soften_color(error, is_dark);

        // Surface variant
        let surface_variant = if is_dark {
            lighten(surface, 0.08)
        } else {
            darken(surface, 0.04)
        };

        // Outline colors
        let outline = if is_dark {
            Color32::from_rgb(120, 120, 120)
        } else {
            Color32::from_rgb(180, 180, 180)
        };

        // Status colors (derive from common conventions)
        let warning = Color32::from_rgb(255, 167, 38);
        let success = Color32::from_rgb(76, 175, 80);
        let info = Color32::from_rgb(33, 150, 243);

        Self {
            // Primary
            primary,
            on_primary,
            primary_container,
            on_primary_container: contrast_color(primary_container),

            // Secondary
            secondary,
            on_secondary,
            secondary_container,
            on_secondary_container: contrast_color(secondary_container),

            // Tertiary
            tertiary,
            on_tertiary,
            tertiary_container,
            on_tertiary_container: contrast_color(tertiary_container),

            // Error
            error,
            on_error,
            error_container,
            on_error_container: contrast_color(error_container),

            // Surface
            surface,
            on_surface,
            surface_variant,
            on_surface_variant: if is_dark {
                Color32::from_rgb(180, 180, 180)
            } else {
                Color32::from_rgb(100, 100, 100)
            },
            outline,
            outline_variant: if is_dark {
                Color32::from_rgb(80, 80, 80)
            } else {
                Color32::from_rgb(220, 220, 220)
            },

            // Status
            warning,
            on_warning: contrast_color(warning),
            success,
            on_success: contrast_color(success),
            info,
            on_info: contrast_color(info),

            // Selection & Interaction
            selection: primary.linear_multiply(0.2),
            inverse_surface: if is_dark {
                Color32::WHITE
            } else {
                Color32::from_rgb(30, 30, 30)
            },
            inverse_on_surface: if is_dark {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
            scrim: Color32::from_rgba_premultiplied(0, 0, 0, 128),
        }
    }

    // =========================================================================
    // PRESET THEMES
    // =========================================================================

    /// Light theme - Y2K Lab Tech
    pub fn light() -> Self {
        crate::themes::y2k_monochrome::light()
    }

    /// Dark theme - Y2K Cyber Void
    pub fn dark() -> Self {
        crate::themes::y2k_monochrome::dark()
    }
}

// =============================================================================
// COLOR UTILITIES
// =============================================================================

/// Get a contrasting color (black or white) based on luminance
fn contrast_color(color: Color32) -> Color32 {
    let luminance = 0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
    if luminance > 128.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// Create a softer/container version of a color
fn soften_color(color: Color32, is_dark: bool) -> Color32 {
    if is_dark {
        // In dark mode, container is a darker, more muted version
        Color32::from_rgb(
            (color.r() as f32 * 0.3) as u8,
            (color.g() as f32 * 0.3) as u8,
            (color.b() as f32 * 0.3) as u8,
        )
    } else {
        // In light mode, container is a lighter version
        Color32::from_rgb(
            (255.0 - (255.0 - color.r() as f32) * 0.15) as u8,
            (255.0 - (255.0 - color.g() as f32) * 0.15) as u8,
            (255.0 - (255.0 - color.b() as f32) * 0.15) as u8,
        )
    }
}

/// Lighten a color by a factor (0.0 - 1.0)
fn lighten(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgb(
        (color.r() as f32 + (255.0 - color.r() as f32) * factor) as u8,
        (color.g() as f32 + (255.0 - color.g() as f32) * factor) as u8,
        (color.b() as f32 + (255.0 - color.b() as f32) * factor) as u8,
    )
}

/// Darken a color by a factor (0.0 - 1.0)
fn darken(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgb(
        (color.r() as f32 * (1.0 - factor)) as u8,
        (color.g() as f32 * (1.0 - factor)) as u8,
        (color.b() as f32 * (1.0 - factor)) as u8,
    )
}
