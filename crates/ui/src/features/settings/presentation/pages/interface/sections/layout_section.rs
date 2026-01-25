//! Layout settings section - configure default view mode and panel visibility.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_widgets::{SegmentedControl, ThemedSlider, ToggleSwitch};
use eframe::egui;
use std::collections::HashMap;

/// Display options from database (key-value pairs)
pub struct LayoutOptions {
    pub default_view_mode: String, // "list" or "grid"
    pub tree_panel_visible: bool,
    pub tree_panel_width: f32,
    pub properties_panel_visible: bool,
    pub properties_panel_width: f32,
}

impl LayoutOptions {
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Self {
            default_view_mode: map
                .get("default_view_mode")
                .cloned()
                .unwrap_or_else(|| "list".to_string()),
            tree_panel_visible: map
                .get("tree_panel_visible")
                .map(|s| s == "true")
                .unwrap_or(true),
            tree_panel_width: map
                .get("tree_panel_width")
                .and_then(|s| s.parse().ok())
                .unwrap_or(200.0),
            properties_panel_visible: map
                .get("properties_panel_visible")
                .map(|s| s == "true")
                .unwrap_or(true),
            properties_panel_width: map
                .get("properties_panel_width")
                .and_then(|s| s.parse().ok())
                .unwrap_or(280.0),
        }
    }

    pub fn to_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert(
            "default_view_mode".to_string(),
            self.default_view_mode.clone(),
        );
        map.insert(
            "tree_panel_visible".to_string(),
            self.tree_panel_visible.to_string(),
        );
        map.insert(
            "tree_panel_width".to_string(),
            self.tree_panel_width.to_string(),
        );
        map.insert(
            "properties_panel_visible".to_string(),
            self.properties_panel_visible.to_string(),
        );
        map.insert(
            "properties_panel_width".to_string(),
            self.properties_panel_width.to_string(),
        );
        map
    }
}

/// Render the layout configuration section
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    options: &mut LayoutOptions,
    on_change: &mut bool,
) {
    // Default view mode
    SectionHeader::new("View Mode").show(ui, &theme.colors);

    let mut is_list = options.default_view_mode == "list";
    SettingsRow::new("Default View")
        .description("Choose how files are displayed by default")
        .action(|ui| {
            if ui
                .add(
                    SegmentedControl::new(&mut is_list, "List", "Grid")
                        .size(100.0, 26.0)
                        .with_theme_colors(&theme.colors),
                )
                .changed()
            {
                options.default_view_mode = if is_list { "list" } else { "grid" }.to_string();
                *on_change = true;
            }
        })
        .show(ui, &theme.colors);

    ui.add_space(12.0);

    // Panel visibility defaults
    SectionHeader::new("Panel Visibility").show(ui, &theme.colors);

    SettingsRow::new("Tree Panel")
        .description("Show folder tree on startup")
        .action(|ui| {
            if ui
                .add(
                    ToggleSwitch::new(&mut options.tree_panel_visible)
                        .size(44.0, 22.0)
                        .with_theme_colors(&theme.colors),
                )
                .changed()
            {
                *on_change = true;
            }
        })
        .show(ui, &theme.colors);

    SettingsRow::new("Properties Panel")
        .description("Show file properties on startup")
        .action(|ui| {
            if ui
                .add(
                    ToggleSwitch::new(&mut options.properties_panel_visible)
                        .size(44.0, 22.0)
                        .with_theme_colors(&theme.colors),
                )
                .changed()
            {
                *on_change = true;
            }
        })
        .show(ui, &theme.colors);

    ui.add_space(12.0);

    // Panel widths
    SectionHeader::new("Panel Widths").show(ui, &theme.colors);

    SettingsRow::new("Tree Panel Width")
        .description("Default width for the folder tree")
        .action(|ui| {
            if ui
                .add(
                    ThemedSlider::new(&mut options.tree_panel_width, 150.0..=400.0)
                        .suffix("px")
                        .width(180.0)
                        .with_theme_colors(&theme.colors),
                )
                .changed()
            {
                *on_change = true;
            }
        })
        .show(ui, &theme.colors);

    SettingsRow::new("Properties Panel Width")
        .description("Default width for file properties")
        .action(|ui| {
            if ui
                .add(
                    ThemedSlider::new(&mut options.properties_panel_width, 200.0..=500.0)
                        .suffix("px")
                        .width(180.0)
                        .with_theme_colors(&theme.colors),
                )
                .changed()
            {
                *on_change = true;
            }
        })
        .show(ui, &theme.colors);
}
