use crate::ThemeColors;
use egui::Color32;

/// Japanese Hacker / Cyberpunk Soft Theme
pub fn theme() -> ThemeColors {
    ThemeColors {
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
