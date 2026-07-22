//! Font loading utilities for CJK and icon support

use egui::Context;
use std::{fs, path::Path};
use tracing::{debug, warn};

/// Load CJK fonts during app initialization to avoid deadlock.
/// This should be called from CreationContext, not during update().
pub fn load_cjk_fonts(ctx: &Context) {
    let cjk_font = load_system_cjk_font();
    let fonts = build_font_definitions(cjk_font.as_ref().map(|(bytes, _)| bytes.clone()));
    ctx.set_fonts(fonts);

    if let Some((_, source)) = cjk_font {
        debug!("Loaded system CJK font from: {}", source);
    } else {
        warn!("No CJK-compatible system font found. Japanese/Chinese characters may not display correctly.");
    }
}

fn build_font_definitions(cjk_font: Option<Vec<u8>>) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    if let Some(font_bytes) = cjk_font {
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
    }

    // Icon registration must not depend on an optional system font.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);

    // Shift icons slightly up to align with the text baseline.
    if let Some(phosphor_font) = fonts.font_data.get_mut("phosphor") {
        let tweak = egui::FontTweak {
            y_offset_factor: -0.15,
            ..Default::default()
        };
        *phosphor_font = std::sync::Arc::new(
            egui::FontData::from_owned(phosphor_font.font.to_vec()).tweak(tweak),
        );
    }

    fonts
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phosphor_is_registered_without_a_system_cjk_font() {
        let fonts = build_font_definitions(None);

        assert!(fonts.font_data.contains_key("phosphor"));
        assert!(fonts
            .families
            .values()
            .flatten()
            .any(|name| name == "phosphor"));
    }

    #[test]
    fn optional_cjk_font_is_prepended_when_available() {
        let fonts = build_font_definitions(Some(vec![0, 1, 2, 3]));

        assert!(fonts.font_data.contains_key("cjk_font"));
        assert!(fonts.font_data.contains_key("phosphor"));
        assert_eq!(
            fonts.families[&egui::FontFamily::Proportional][0],
            "cjk_font"
        );
    }
}
