use crate::features::settings::types::SettingsAction;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_core::UiService;
use arclain_core::{UiItem, UiRegion};
use eframe::egui;

use super::sections;

/// State for interface settings loaded from database
pub struct InterfaceSettingsState {
    pub items: Vec<UiItem>,
    pub layout_options: sections::layout_section::LayoutOptions,
    pub dirty: bool,
    pub loaded: bool,
    /// Show the layout type selection dialog
    pub layout_dialog_open: bool,
}

impl Default for InterfaceSettingsState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            layout_options: sections::layout_section::LayoutOptions {
                default_view_mode: "list".to_string(),
                tree_panel_visible: true,
                tree_panel_width: 200.0,
                properties_panel_visible: true,
                properties_panel_width: 280.0,
            },
            dirty: false,
            loaded: false,
            layout_dialog_open: false,
        }
    }
}

impl InterfaceSettingsState {
    /// Load items from database via UiService
    pub fn load_from_service(&mut self, service: &UiService) {
        if self.loaded {
            return;
        }

        // Load all items from all regions
        self.items.clear();
        for region in [
            UiRegion::Toolbar,
            UiRegion::ContextMenu,
            UiRegion::InfoPanel,
            UiRegion::ToolsDialog,
        ] {
            if let Ok(items) = service.list_items(region) {
                self.items.extend(items);
            }
        }

        // Load display options
        let mut opts_map = std::collections::HashMap::new();
        for key in [
            "default_view_mode",
            "tree_panel_visible",
            "tree_panel_width",
            "properties_panel_visible",
            "properties_panel_width",
        ] {
            if let Ok(Some(val)) = service.get_display_option(key) {
                opts_map.insert(key.to_string(), val);
            }
        }
        self.layout_options = sections::layout_section::LayoutOptions::from_map(&opts_map);

        self.loaded = true;
        self.dirty = false;
    }

    /// Save modified items back to database via UiService
    pub fn save_to_service(&mut self, service: &UiService) {
        if !self.dirty {
            return;
        }

        // Save all items
        let _ = service.upsert_items(&self.items);

        // Save display options
        for (key, value) in self.layout_options.to_map() {
            let _ = service.set_display_option(&key, &value);
        }

        self.dirty = false;
    }
}

/// Render the Interface settings page
/// Returns Some(SettingsAction) if navigation is requested
pub fn render_interface_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    shared: &SharedState,
    interface_state: &mut InterfaceSettingsState,
    ui_service: Option<&UiService>,
) -> Option<SettingsAction> {
    let mut action: Option<SettingsAction> = None;

    // Load items from database if not already loaded
    if !interface_state.loaded {
        if let Some(service) = ui_service {
            interface_state.load_from_service(service);
        }
    }

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Toolbar section - just Edit Layout button
        render_section(ui, theme, "Toolbar", |ui| {
            ui.label(
                egui::RichText::new("Customize toolbar button arrangement")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            // Edit Layout button - opens dialog
            if ui
                .button(format!(
                    "{} Edit Layout",
                    egui_phosphor::regular::PENCIL_SIMPLE
                ))
                .clicked()
            {
                interface_state.layout_dialog_open = true;
            }
        });

        ui.add_space(8.0);

        // Context Menu section
        render_section(ui, theme, "Context Menu", |ui| {
            sections::context_menu_section::render(
                ui,
                theme,
                &mut interface_state.items,
                &mut interface_state.dirty,
            );
        });

        ui.add_space(8.0);

        // Info Panel section
        render_section(ui, theme, "Info Panel", |ui| {
            sections::info_panel_section::render(
                ui,
                theme,
                &mut interface_state.items,
                &mut interface_state.dirty,
            );
        });

        ui.add_space(8.0);

        // Layout section
        render_section(ui, theme, "Layout", |ui| {
            sections::layout_section::render(
                ui,
                theme,
                &mut interface_state.layout_options,
                &mut interface_state.dirty,
            );
        });

        ui.add_space(8.0);

        // Legacy: Show button labels toggle (for header)
        render_section(ui, theme, "Header", |ui| {
            ui.label(
                egui::RichText::new("Configure header button display")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            let show_labels = shared.signals().ui_preferences.get().show_button_labels;
            let mut show_labels_mut = show_labels;

            if ui
                .checkbox(&mut show_labels_mut, "Show button labels in header")
                .on_hover_text("Display text labels next to icons in header buttons")
                .changed()
            {
                let mut prefs = shared.signals().ui_preferences.get();
                prefs.show_button_labels = show_labels_mut;
                shared.signals().ui_preferences.set(prefs);
            }
        });

        ui.add_space(16.0);

        // Auto-save: save immediately when changes are made
        if interface_state.dirty {
            if let Some(service) = ui_service {
                interface_state.save_to_service(service);

                // Reload toolbar items in AppSignals
                if let Ok(items) = service.list_toolbar_items() {
                    shared.signals().toolbar_items.set(items);
                }

                // Reload info panel items in AppSignals
                if let Ok(items) = service.list_info_panel_items() {
                    shared.signals().info_panel_items.set(items);
                }
            }
        }
    });

    // Layout type selection dialog
    if interface_state.layout_dialog_open {
        egui::Window::new("Choose Layout to Edit")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_min_width(280.0);

                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("What would you like to customize?")
                            .size(14.0)
                            .color(theme.colors.on_surface),
                    );
                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        // Toolbar button
                        if ui
                            .add_sized(
                                [120.0, 40.0],
                                egui::Button::new(format!(
                                    "{} Toolbar",
                                    egui_phosphor::regular::STACK
                                )),
                            )
                            .clicked()
                        {
                            interface_state.layout_dialog_open = false;
                            action = Some(SettingsAction::NavigateTo(
                                crate::core::navigation::SettingsPage::ToolbarLayout,
                            ));
                        }

                        ui.add_space(16.0);

                        // Info Panel button
                        if ui
                            .add_sized(
                                [120.0, 40.0],
                                egui::Button::new(format!(
                                    "{} Info Panel",
                                    egui_phosphor::regular::SIDEBAR
                                )),
                            )
                            .clicked()
                        {
                            interface_state.layout_dialog_open = false;
                            action = Some(SettingsAction::NavigateTo(
                                crate::core::navigation::SettingsPage::InfoPanelLayout,
                            ));
                        }
                    });

                    ui.add_space(12.0);

                    // Cancel button
                    if ui.button("Cancel").clicked() {
                        interface_state.layout_dialog_open = false;
                    }
                });
            });
    }

    action
}

/// Helper function to render a settings section with consistent styling
fn render_section<R>(
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
