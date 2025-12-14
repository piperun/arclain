//! Theme colors implementing Semantic Naming (Material-like)

use egui::Color32;

/// Theme colors implementing Semantic Naming (Material-like)
#[derive(Clone, Debug)]
pub struct ThemeColors {
    // --- Primary Colors ---
    /// The primary color used for major UI elements (buttons, active states)
    pub primary: Color32,
    /// Content color drawn on top of the primary color
    pub on_primary: Color32,

    // --- Secondary Colors ---
    /// Secondary color for less prominent elements
    pub secondary: Color32,
    /// Content color drawn on top of the secondary color
    pub on_secondary: Color32,

    // --- Surface Colors ---
    /// The background color of the main window/surface
    pub surface: Color32,
    /// Content color drawn on top of the surface
    pub on_surface: Color32,
    /// A variant of the surface color (e.g. for panels or cards)
    pub surface_variant: Color32,
    /// Content color drawn on top of the surface variant
    pub on_surface_variant: Color32,

    // --- Outline ---
    /// Color for borders and dividers
    pub outline: Color32,

    // --- Error ---
    pub error: Color32,
    pub on_error: Color32,

    // --- Selection ---
    pub selection: Color32,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self::light()
    }
}

impl ThemeColors {
    /// Light theme - Japanese Minimalist
    pub fn light() -> Self {
        Self {
            primary: Color32::from_rgb(0, 0, 0),
            on_primary: Color32::WHITE,
            secondary: Color32::from_rgb(248, 249, 250),
            on_secondary: Color32::from_rgb(33, 37, 41),
            surface: Color32::WHITE,
            on_surface: Color32::from_rgb(33, 37, 41),
            surface_variant: Color32::from_rgb(248, 249, 250),
            on_surface_variant: Color32::from_rgb(73, 80, 87),
            outline: Color32::from_rgb(222, 226, 230),
            error: Color32::from_rgb(176, 0, 32),
            on_error: Color32::WHITE,
            selection: Color32::from_rgba_premultiplied(0, 0, 0, 20),
        }
    }

    /// Dark theme - Japanese Hacker/Cyberpunk
    pub fn dark() -> Self {
        Self {
            primary: Color32::WHITE,
            on_primary: Color32::BLACK,
            secondary: Color32::from_rgb(20, 20, 20),
            on_secondary: Color32::from_rgb(224, 224, 224),
            surface: Color32::from_rgb(10, 10, 10),
            on_surface: Color32::WHITE,
            surface_variant: Color32::from_rgb(20, 20, 20),
            on_surface_variant: Color32::from_rgb(153, 153, 153),
            outline: Color32::from_rgb(42, 42, 42),
            error: Color32::from_rgb(207, 102, 121),
            on_error: Color32::BLACK,
            selection: Color32::from_rgba_premultiplied(255, 255, 255, 30),
        }
    }
}
