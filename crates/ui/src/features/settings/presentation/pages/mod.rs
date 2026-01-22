//! Settings Pages Module
//!
//! Contains individual settings page implementations.

pub mod archives;
pub mod general;
pub mod interface;
pub mod keyboard_mouse;
pub mod layout_editor;
pub mod network;
pub mod organization_rules;
pub mod plugins;
pub mod security;

// Re-export for convenience
pub use interface::render_interface_settings;
pub use layout_editor::{
    render_info_panel_layout, render_toolbar_layout, InfoPanelLayoutState, ToolbarLayoutState,
};
pub use organization_rules::RulesPage;

use crate::shared::theme::AppTheme;
use eframe::egui;

/// Helper function to render a settings section with consistent styling
pub fn render_settings_section<R>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .corner_radius(8.0)
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );
                ui.add_space(8.0);
                content(ui)
            })
            .inner
        })
        .inner
}
