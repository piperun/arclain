//! Interface settings page.
//!
//! Architecture: `render_interface_settings` returns
//! `Option<InterfaceSettingsAction>` describing intent — either a
//! data load, a save-and-sync cascade, or a navigation request. The
//! sibling `handle_interface_settings_action` function owns all
//! UiService calls and signal mutations so the render path itself
//! stays a pure intent-emitter.

use crate::shared::components::Form;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_core::UiService;
use arclain_core::{UiItem, UiRegion};
use arclain_theme::spacing;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

use super::sections;

/// State for interface settings loaded from database
pub struct InterfaceSettingsState {
    pub items: Vec<UiItem>,
    pub layout_options: sections::layout_section::LayoutOptions,
    pub show_button_labels: bool,
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
            show_button_labels: false,
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
            "show_button_labels",
        ] {
            if let Ok(Some(val)) = service.get_display_option(key) {
                opts_map.insert(key.to_string(), val);
            }
        }
        self.layout_options = sections::layout_section::LayoutOptions::from_map(&opts_map);

        // Load show_button_labels
        self.show_button_labels = opts_map
            .get("show_button_labels")
            .map(|s| s == "true")
            .unwrap_or(false);

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

        // Save show_button_labels
        let _ = service.set_display_option(
            "show_button_labels",
            &self.show_button_labels.to_string(),
        );

        self.dirty = false;
    }
}

/// Intents emitted by `render_interface_settings`. Caller translates
/// `Navigate` into a `SettingsAction::NavigateTo` and routes data
/// actions through `handle_interface_settings_action`.
#[derive(Debug, Clone)]
pub enum InterfaceSettingsAction {
    /// First-render load: fetch items + display options from the
    /// UiService and populate `state`. Auto-fired when `state.loaded`
    /// is false.
    LoadItems,
    /// User mutated something (`state.dirty` is true). Save items +
    /// display options to the UiService, then refresh the canonical
    /// signals (`toolbar_items`, `info_panel_items`, `ui_preferences`,
    /// `browser_view_state`) so other features see the update.
    SaveAndSync,
    /// User picked a layout target in the dialog — navigate to its
    /// editor page.
    Navigate(crate::core::navigation::SettingsPage),
}

/// Render the Interface settings page.
pub fn render_interface_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    interface_state: &mut InterfaceSettingsState,
) -> Option<InterfaceSettingsAction> {
    // Auto-fire LoadItems on first render. Dispatcher populates
    // state.items + state.layout_options + state.show_button_labels
    // synchronously after render returns; next frame the form
    // renders against real data.
    if !interface_state.loaded {
        ui.label(
            egui::RichText::new("Loading interface settings…")
                .size(12.0)
                .color(theme.colors.on_surface_variant),
        );
        return Some(InterfaceSettingsAction::LoadItems);
    }

    let mut emitted: Option<InterfaceSettingsAction> = None;

    Form::new()
        .id("interface_settings")
        .show(ui, theme, |ui| {
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
                    .add(
                        TextButton::new(
                            format!(
                                "{} Edit Layout",
                                egui_phosphor::regular::PENCIL_SIMPLE
                            ),
                            ButtonSize::Medium,
                        )
                        .with_theme_colors(&theme.colors),
                    )
                    .clicked()
                {
                    interface_state.layout_dialog_open = true;
                }
            });

            // Context Menu section
            render_section(ui, theme, "Context Menu", |ui| {
                sections::context_menu_section::render(
                    ui,
                    theme,
                    &mut interface_state.items,
                    &mut interface_state.dirty,
                );
            });

            // Info Panel section
            render_section(ui, theme, "Info Panel", |ui| {
                sections::info_panel_section::render(
                    ui,
                    theme,
                    &mut interface_state.items,
                    &mut interface_state.dirty,
                );
            });

            // Layout section
            render_section(ui, theme, "Layout", |ui| {
                sections::layout_section::render(
                    ui,
                    theme,
                    &mut interface_state.layout_options,
                    &mut interface_state.dirty,
                );
            });

            // Header section - button labels
            render_section(ui, theme, "Header", |ui| {
                ui.label(
                    egui::RichText::new("Configure header button display")
                        .size(12.0)
                        .color(theme.colors.on_surface_variant),
                );
                ui.add_space(8.0);

                if ui
                    .checkbox(&mut interface_state.show_button_labels, "Show button labels in header")
                    .on_hover_text("Display text labels next to icons in header buttons")
                    .changed()
                {
                    interface_state.dirty = true;
                }
            });

            ui.add_space(16.0);
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
                            emitted = Some(InterfaceSettingsAction::Navigate(
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
                            emitted = Some(InterfaceSettingsAction::Navigate(
                                crate::core::navigation::SettingsPage::InfoPanelLayout,
                            ));
                        }
                    });

                    ui.add_space(12.0);

                    // Cancel button
                    if ui
                        .add(
                            TextButton::new("Cancel", ButtonSize::Small)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        interface_state.layout_dialog_open = false;
                    }
                });
            });
    }

    // Auto-save: any pending dirty state takes priority over navigation
    // dispatch on the same frame ONLY if no Navigate was emitted —
    // otherwise the next frame will see dirty still true and re-emit.
    if emitted.is_none() && interface_state.dirty {
        emitted = Some(InterfaceSettingsAction::SaveAndSync);
    }

    emitted
}

/// Dispatch an `InterfaceSettingsAction` against the UiService and
/// the shared signal graph. Called by the parent view after
/// `render_interface_settings` returns an action. All side effects on
/// the DB and on canonical signals live here.
pub fn handle_interface_settings_action(
    state: &mut InterfaceSettingsState,
    action: InterfaceSettingsAction,
    shared: &SharedState,
) {
    match action {
        InterfaceSettingsAction::Navigate(_) => {
            // Navigation is the caller's responsibility — it translates
            // to SettingsAction::NavigateTo and returns up the chain.
            // The dispatcher should never be called with this variant.
            debug_assert!(
                false,
                "InterfaceSettingsAction::Navigate should be handled by the caller, not the data dispatcher"
            );
        }
        InterfaceSettingsAction::LoadItems => {
            if let Some(service) = shared.services.ui_service.as_deref() {
                state.load_from_service(service);
            }
        }
        InterfaceSettingsAction::SaveAndSync => {
            let Some(service) = shared.services.ui_service.as_deref() else {
                return;
            };
            state.save_to_service(service);

            // Refresh canonical item signals so anyone subscribed
            // (header, browser, etc.) sees the update.
            if let Ok(items) = service.list_toolbar_items() {
                shared.signals().toolbar_items.set(items);
            }
            if let Ok(items) = service.list_info_panel_items() {
                shared.signals().info_panel_items.set(items);
            }

            // Push label preference into ui_preferences signal.
            let mut prefs = shared.signals().ui_preferences.get();
            prefs.show_button_labels = state.show_button_labels;
            shared.signals().ui_preferences.set(prefs);

            // Push panel-visibility into the active tab's browser
            // view state. Mirrors the pre-MVU auto-save behavior:
            // applying a Layout setting takes effect on the visible
            // tab immediately so the user sees the change.
            shared
                .signals()
                .tabs
                .get()
                .active()
                .browser_view_state
                .update(|s| {
                    s.toolbar_state.show_tree_panel = state.layout_options.tree_panel_visible;
                    s.toolbar_state.show_properties_panel =
                        state.layout_options.properties_panel_visible;
                });
        }
    }
}

/// Helper function to render a settings section with consistent Y2K styling
fn render_section<R>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))  // Keep border
        .corner_radius(egui::CornerRadius::ZERO)                // Y2K: zero radius
        .inner_margin(spacing::CARD)
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
