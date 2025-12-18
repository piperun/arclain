use crate::shared::theme::AppTheme;
use arclain_db::{UiItem, UiRegion};
use eframe::egui;

use super::sections;

/// State for interface settings loaded from database
pub struct InterfaceSettingsState {
    pub items: Vec<UiItem>,
    pub layout_options: sections::layout_section::LayoutOptions,
    pub dirty: bool,
    pub loaded: bool,
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
        }
    }
}

impl InterfaceSettingsState {
    /// Load items from database
    pub fn load_from_db(&mut self, conn: &rusqlite::Connection) {
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
            if let Ok(items) = arclain_db::list_items_by_region(conn, region) {
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
            if let Ok(Some(val)) = arclain_db::get_display_option(conn, key) {
                opts_map.insert(key.to_string(), val);
            }
        }
        self.layout_options = sections::layout_section::LayoutOptions::from_map(&opts_map);

        self.loaded = true;
        self.dirty = false;
    }

    /// Save modified items back to database
    pub fn save_to_db(&mut self, conn: &rusqlite::Connection) {
        if !self.dirty {
            return;
        }

        // Save all items
        for item in &self.items {
            let _ = arclain_db::upsert_item(conn, item);
        }

        // Save display options
        for (key, value) in self.layout_options.to_map() {
            let _ = arclain_db::set_display_option(conn, &key, &value);
        }

        self.dirty = false;
    }
}

/// Render the Interface settings page
/// Returns true if settings were saved
pub fn render_interface_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    app_state: &std::sync::Arc<parking_lot::Mutex<crate::core::AppState>>,
    interface_state: &mut InterfaceSettingsState,
) -> bool {
    let mut saved = false;

    // Load items from database if not already loaded
    if !interface_state.loaded {
        let conn_opt = {
            let state = app_state.lock();
            state.dbs.as_ref().map(|dbs| dbs.config.connection())
        };
        if let Some(conn) = conn_opt {
            if let Ok(guard) = conn.lock() {
                interface_state.load_from_db(&guard);
            }
        }
    }

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Toolbar section
        render_section(ui, theme, "Toolbar", |ui| {
            sections::toolbar_section::render(
                ui,
                theme,
                &mut interface_state.items,
                &mut interface_state.dirty,
            );
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

            let mut show_labels = {
                let state = app_state.lock();
                state.ui_preferences.show_button_labels
            };

            if ui
                .checkbox(&mut show_labels, "Show button labels in header")
                .on_hover_text("Display text labels next to icons in header buttons")
                .changed()
            {
                let mut state = app_state.lock();
                state.ui_preferences.show_button_labels = show_labels;
            }
        });

        ui.add_space(16.0);

        // Auto-save: save immediately when changes are made
        if interface_state.dirty {
            let conn_opt = {
                let state = app_state.lock();
                state.dbs.as_ref().map(|dbs| dbs.config.connection())
            };
            if let Some(conn) = conn_opt {
                if let Ok(guard) = conn.lock() {
                    interface_state.save_to_db(&guard);

                    // Reload toolbar items in AppState so changes take effect immediately
                    if let Ok(items) = arclain_db::list_items_by_region(&guard, UiRegion::Toolbar) {
                        let mut state = app_state.lock();
                        state.toolbar_items = items;
                    }

                    saved = true;
                }
            }
        }
    });

    saved
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
