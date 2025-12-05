use eframe::egui;
use std::{fs, path::Path};
use tracing::{debug, warn};

/// Theme colors matching the mockup design
#[derive(Clone, Debug)]
pub struct ThemeColors {
    // Backgrounds
    pub bg_primary: egui::Color32,
    pub bg_secondary: egui::Color32,
    pub bg_tertiary: egui::Color32,
    pub bg_hover: egui::Color32,

    // Text
    pub text_primary: egui::Color32,
    pub text_secondary: egui::Color32,
    pub text_muted: egui::Color32,

    // Borders
    pub border_color: egui::Color32,
    pub border_light: egui::Color32,

    // Accents
    pub accent: egui::Color32,
    pub accent_hover: egui::Color32,
    pub selection: egui::Color32,
    pub selection_text: egui::Color32,
}

impl ThemeColors {
    /// Light theme - Japanese Minimalist
    pub fn light() -> Self {
        Self {
            bg_primary: egui::Color32::from_rgb(255, 255, 255), // #ffffff
            bg_secondary: egui::Color32::from_rgb(248, 249, 250), // #f8f9fa
            bg_tertiary: egui::Color32::from_rgb(233, 236, 239), // #e9ecef
            bg_hover: egui::Color32::from_rgb(222, 226, 230),   // #dee2e6

            text_primary: egui::Color32::from_rgb(33, 37, 41), // #212529
            text_secondary: egui::Color32::from_rgb(73, 80, 87), // #495057
            text_muted: egui::Color32::from_rgb(108, 117, 125), // #6c757d

            border_color: egui::Color32::from_rgb(222, 226, 230), // #dee2e6
            border_light: egui::Color32::from_rgb(233, 236, 239), // #e9ecef

            accent: egui::Color32::from_rgb(0, 0, 0), // #000000
            accent_hover: egui::Color32::from_rgb(33, 37, 41), // #212529
            selection: egui::Color32::from_rgba_premultiplied(0, 0, 0, 20), // rgba(0, 0, 0, 0.08)
            selection_text: egui::Color32::from_rgb(0, 0, 0), // Black text on light selection
        }
    }

    /// Dark theme - Japanese Hacker/Cyberpunk
    pub fn dark() -> Self {
        Self {
            bg_primary: egui::Color32::from_rgb(10, 10, 10), // #0a0a0a
            bg_secondary: egui::Color32::from_rgb(20, 20, 20), // #141414
            bg_tertiary: egui::Color32::from_rgb(26, 26, 26), // #1a1a1a
            bg_hover: egui::Color32::from_rgb(38, 38, 38),   // #262626

            text_primary: egui::Color32::from_rgb(255, 255, 255), // #ffffff
            text_secondary: egui::Color32::from_rgb(224, 224, 224), // #e0e0e0
            text_muted: egui::Color32::from_rgb(153, 153, 153),   // #999999

            border_color: egui::Color32::from_rgb(42, 42, 42), // #2a2a2a
            border_light: egui::Color32::from_rgb(31, 31, 31), // #1f1f1f

            accent: egui::Color32::from_rgb(255, 255, 255), // #ffffff
            accent_hover: egui::Color32::from_rgb(224, 224, 224), // #e0e0e0
            selection: egui::Color32::from_rgb(255, 255, 255), // Solid white for dark mode
            selection_text: egui::Color32::from_rgb(0, 0, 0), // Black text on white selection
        }
    }
}

#[derive(Clone)]
pub struct AppTheme {
    pub colors: ThemeColors,
    pub dark_mode: bool,
}

impl AppTheme {
    pub fn new(dark_mode: bool) -> Self {
        Self {
            colors: if dark_mode {
                ThemeColors::dark()
            } else {
                ThemeColors::light()
            },
            dark_mode,
        }
    }

    pub fn toggle(&mut self) {
        self.dark_mode = !self.dark_mode;
        self.colors = if self.dark_mode {
            ThemeColors::dark()
        } else {
            ThemeColors::light()
        };
    }

    pub fn apply_to_context(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        // Customize visuals to match mockup
        visuals.widgets.noninteractive.bg_fill = self.colors.bg_primary;
        visuals.widgets.noninteractive.weak_bg_fill = self.colors.bg_secondary;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, self.colors.border_color);
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.colors.text_primary);

        visuals.widgets.inactive.bg_fill = self.colors.bg_tertiary;
        visuals.widgets.inactive.weak_bg_fill = self.colors.bg_secondary;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, self.colors.border_color);
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, self.colors.text_primary);

        visuals.widgets.hovered.bg_fill = self.colors.bg_hover;
        visuals.widgets.hovered.weak_bg_fill = self.colors.bg_hover;
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, self.colors.border_color);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, self.colors.text_primary);

        visuals.widgets.active.bg_fill = self.colors.accent_hover;
        visuals.widgets.active.weak_bg_fill = self.colors.bg_hover;
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, self.colors.accent);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, self.colors.text_primary);

        visuals.selection.bg_fill = self.colors.selection;
        visuals.selection.stroke = egui::Stroke::new(1.0, self.colors.accent);

        visuals.window_fill = self.colors.bg_primary;
        visuals.panel_fill = self.colors.bg_secondary;
        visuals.faint_bg_color = self.colors.bg_secondary;
        visuals.extreme_bg_color = self.colors.bg_secondary;

        visuals.window_stroke = egui::Stroke::new(1.0, self.colors.border_color);
        visuals.window_corner_radius = egui::CornerRadius::same(8);

        // Override text colors
        visuals.override_text_color = Some(self.colors.text_primary);

        ctx.set_visuals(visuals);
    }

    /// Custom button style matching mockup
    pub fn button_style(&self) -> egui::style::WidgetVisuals {
        egui::style::WidgetVisuals {
            bg_fill: egui::Color32::TRANSPARENT,
            weak_bg_fill: self.colors.bg_secondary,
            bg_stroke: egui::Stroke::NONE,
            fg_stroke: egui::Stroke::new(1.0, self.colors.text_secondary),
            corner_radius: egui::CornerRadius::same(4),
            expansion: 0.0,
        }
    }
}

/// Load CJK fonts during app initialization to avoid deadlock
/// This should be called from CreationContext, not during update()
pub fn load_cjk_fonts(ctx: &egui::Context) {
    if let Some((font_bytes, source)) = load_system_cjk_font() {
        let mut fonts = egui::FontDefinitions::default();

        fonts.font_data.insert(
            "cjk_font".to_string(),
            std::sync::Arc::new(egui::FontData::from_owned(font_bytes)),
        );

        // Insert CJK font at the beginning of the proportional family
        // so it's used as a fallback for missing glyphs
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk_font".to_string());

        // Also add to monospace family
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk_font".to_string());

        // Add Phosphor icons
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

        ctx.set_fonts(fonts);
        debug!("Loaded system CJK font from: {}", source);
    } else {
        warn!("No CJK-compatible system font found. Japanese/Chinese characters may not display correctly.");
    }
}

fn load_system_cjk_font() -> Option<(Vec<u8>, String)> {
    for candidate in system_font_candidates() {
        let font_path = Path::new(candidate);
        if font_path.exists() {
            match fs::read(font_path) {
                Ok(bytes) => return Some((bytes, font_path.display().to_string())),
                Err(err) => warn!("Failed to read font {}: {}", font_path.display(), err),
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn system_font_candidates() -> &'static [&'static str] {
    &[
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\YuGothR.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\MSGOTHIC.TTC",
        "C:\\Windows\\Fonts\\meiryo.ttc",
    ]
}

#[cfg(target_os = "macos")]
fn system_font_candidates() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Supplemental/ArialUnicodeMS.ttf",
    ]
}

#[cfg(target_os = "linux")]
fn system_font_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ]
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn system_font_candidates() -> &'static [&'static str] {
    &[]
}
