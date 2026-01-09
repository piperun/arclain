//! Settings Page Layout
//!
//! Handles the main two-panel layout (Navigation + Content)

use eframe::egui;
use egui_extras::{Size, StripBuilder};

/// Render the main settings layout strip
///
/// - `nav_ui`: Closure to render the navigation panel
/// - `content_ui`: Closure to render the content panel (header + body)
pub fn render_settings_layout(
    ui: &mut egui::Ui,
    theme: &crate::shared::theme::AppTheme,
    nav_ui: impl FnOnce(&mut egui::Ui),
    content_ui: impl FnOnce(&mut egui::Ui),
) {
    StripBuilder::new(ui)
        .size(Size::exact(250.0)) // Navigation width
        .size(Size::remainder()) // Content width
        .horizontal(|mut strip| {
            // Strip 1: Navigation
            strip.cell(|ui| {
                ui.push_id("settings_nav_strip", |ui| {
                    // Mimic SidePanel styling
                    egui::Frame::side_top_panel(ui.style())
                        .fill(theme.colors.surface_variant)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_height(ui.available_height());
                            nav_ui(ui);
                        });
                });
            });

            // Strip 2: Content (Header / Body)
            strip.cell(|ui| {
                ui.push_id("settings_content_strip", |ui| {
                    content_ui(ui);
                });
            });
        });
}
