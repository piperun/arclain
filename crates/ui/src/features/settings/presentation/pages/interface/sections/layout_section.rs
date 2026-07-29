//! Layout settings section - configure default view mode and panel visibility.
//!
//! Renders the five layout fields of the application's own
//! `UiDisplayOptionsDto` in place. It deliberately does not own a
//! separate copy of them: this section used to hold a `LayoutOptions`
//! struct that parsed and re-serialized the stored key/value text
//! itself, which made the frontend a second authority on what an unset
//! or unparseable option means. The application answers that now.
//!
//! `show_button_labels` is the one field of that value this section does
//! not touch — the Interface page's own Header section renders it.

use crate::shared::components::settings_form::{SectionHeader, SettingsRow};
use crate::shared::theme::AppTheme;
use arclain_app::layout::{UiDisplayOptionsDto, UiViewModeDto};
use arclain_widgets::{SegmentedControl, ThemedSlider, ToggleSwitch};
use eframe::egui;

/// Render the layout configuration section
pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    options: &mut UiDisplayOptionsDto,
    on_change: &mut bool,
) {
    // Default view mode
    SectionHeader::new("View Mode").show(ui, &theme.colors);

    let mut is_list = options.default_view_mode == UiViewModeDto::List;
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
                options.default_view_mode = if is_list {
                    UiViewModeDto::List
                } else {
                    UiViewModeDto::Grid
                };
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
