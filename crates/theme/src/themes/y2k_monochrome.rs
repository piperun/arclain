//! Y2K Monochrome Theme
//!
//! A Japanese Y2K-inspired strict monochrome theme.
//! - Zero border radius (razor sharp)
//! - High contrast inversion for active states
//! - Only black/white/grey values

use crate::ThemeColors;
use egui::Color32;

/// Y2K Monochrome Dark Theme - "Cyber Void"
/// Deep black base with pure white signal
pub fn dark() -> ThemeColors {
    // 4-color palette for dark mode
    let void = Color32::from_rgb(5, 5, 5); // #050505 - deepest background
    let plate = Color32::from_rgb(0, 0, 0); // #000000 - pure black panels
    let grid = Color32::from_rgb(51, 51, 51); // #333333 - structure lines
    let signal = Color32::from_rgb(255, 255, 255); // #FFFFFF - pure white text/active

    // Secondary greys for hierarchy
    let dim = Color32::from_rgb(128, 128, 128); // #808080 - dimmed/secondary text
    let subtle = Color32::from_rgb(26, 26, 26); // #1A1A1A - subtle surface lift

    ThemeColors {
        // Primary - White is the "signal" color
        primary: signal,
        on_primary: plate,
        primary_container: subtle,
        on_primary_container: signal,

        // Secondary - Mid grey
        secondary: grid,
        on_secondary: signal,
        secondary_container: subtle,
        on_secondary_container: dim,

        // Tertiary - Same as secondary for monochrome
        tertiary: grid,
        on_tertiary: signal,
        tertiary_container: subtle,
        on_tertiary_container: dim,

        // Error - Still need some distinction for errors
        error: Color32::from_rgb(200, 50, 50),
        on_error: signal,
        error_container: Color32::from_rgb(80, 20, 20),
        on_error_container: Color32::from_rgb(255, 150, 150),

        // Surfaces - The core Y2K look
        surface: void,
        on_surface: signal,
        surface_variant: plate,
        on_surface_variant: dim,
        outline: grid,
        outline_variant: subtle,

        // Status colors (monochrome variants)
        warning: Color32::from_rgb(200, 200, 50),
        on_warning: plate,
        success: Color32::from_rgb(50, 200, 50),
        on_success: plate,
        info: dim,
        on_info: signal,

        // Selection - Inverted (white bg, black text)
        selection: signal.linear_multiply(0.15),
        inverse_surface: signal,
        inverse_on_surface: plate,
        scrim: Color32::from_rgba_premultiplied(0, 0, 0, 200),
    }
}

/// Y2K Monochrome Light Theme - "Lab Tech"
/// Clean white base with pure black ink
pub fn light() -> ThemeColors {
    // 4-color palette for light mode
    let lab = Color32::from_rgb(240, 240, 240); // #F0F0F0 - tech grey base
    let paper = Color32::from_rgb(255, 255, 255); // #FFFFFF - pure white panels
    let ink = Color32::from_rgb(0, 0, 0); // #000000 - pure black structure
    let lead = Color32::from_rgb(0, 0, 0); // #000000 - pure black text

    // Secondary greys
    let dim = Color32::from_rgb(100, 100, 100); // #646464 - secondary text
    let soft = Color32::from_rgb(220, 220, 220); // #DCDCDC - soft borders

    ThemeColors {
        // Primary - Black is the "ink" color
        primary: ink,
        on_primary: paper,
        primary_container: soft,
        on_primary_container: ink,

        // Secondary
        secondary: dim,
        on_secondary: paper,
        secondary_container: lab,
        on_secondary_container: ink,

        // Tertiary
        tertiary: dim,
        on_tertiary: paper,
        tertiary_container: lab,
        on_tertiary_container: ink,

        // Error
        error: Color32::from_rgb(180, 30, 30),
        on_error: paper,
        error_container: Color32::from_rgb(255, 220, 220),
        on_error_container: Color32::from_rgb(100, 0, 0),

        // Surfaces
        surface: lab,
        on_surface: lead,
        surface_variant: paper,
        on_surface_variant: dim,
        outline: ink,
        outline_variant: soft,

        // Status
        warning: Color32::from_rgb(180, 150, 0),
        on_warning: paper,
        success: Color32::from_rgb(0, 150, 0),
        on_success: paper,
        info: dim,
        on_info: paper,

        // Selection
        selection: ink.linear_multiply(0.1),
        inverse_surface: ink,
        inverse_on_surface: paper,
        scrim: Color32::from_rgba_premultiplied(0, 0, 0, 128),
    }
}
