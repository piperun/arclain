use crate::ThemeColors;
use egui::Color32;

/// Japanese Hacker / Cyberpunk Soft Theme
pub fn theme() -> ThemeColors {
    // Use from_seed for auto-derivation of container/status colors
    ThemeColors::from_seed(
        Color32::WHITE,                   // primary: white
        Color32::from_rgb(100, 100, 100), // secondary: gray
        Color32::from_rgb(80, 150, 180),  // tertiary: cyan accent
        Color32::from_rgb(10, 10, 10),    // surface: near-black
        true,                             // is_dark
    )
}
