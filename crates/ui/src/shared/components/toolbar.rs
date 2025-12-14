use crate::shared::theme::AppTheme;
use eframe::egui;

pub struct ToolbarState {
    pub show_tree_panel: bool,
    pub show_properties_panel: bool,
    pub grid_view: bool,
    pub columns_locked: bool,
}

impl Default for ToolbarState {
    fn default() -> Self {
        Self {
            show_tree_panel: true,
            show_properties_panel: true,
            grid_view: false,
            columns_locked: true, // Start with columns locked to ensure proper default widths
        }
    }
}

#[derive(Default)]
pub struct ToolbarActions {
    pub go_back: bool,
    pub go_forward: bool,
    pub go_up: bool,
    pub extract: bool,
    pub extract_all: bool,
    pub add: bool,
    pub open: bool,
    pub delete_selected: bool,
    pub convert_to_7z: bool,
    pub organize_archive: bool,
}

pub fn render(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    state: &mut ToolbarState,
    can_go_back: bool,
    can_go_forward: bool,
    can_go_up: bool,
    archive_loaded: bool,
    has_selection: bool,
    _has_metadata: bool,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Navigation group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if toolbar_button(ui, theme, egui_phosphor::regular::ARROW_LEFT, can_go_back) {
                    actions.go_back = true;
                }
                if toolbar_button(
                    ui,
                    theme,
                    egui_phosphor::regular::ARROW_RIGHT,
                    can_go_forward,
                ) {
                    actions.go_forward = true;
                }
                if toolbar_button(ui, theme, egui_phosphor::regular::ARROW_UP, can_go_up) {
                    actions.go_up = true;
                }
            });
        });

        ui.add_space(4.0);

        // File actions group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::FOLDER_OPEN,
                    "Open",
                    true,
                ) {
                    actions.open = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::EXPORT,
                    "Extract",
                    archive_loaded && has_selection,
                ) {
                    actions.extract = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::EXPORT,
                    "Extract all",
                    archive_loaded,
                ) {
                    actions.extract_all = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::PLUS,
                    "Add",
                    archive_loaded,
                ) {
                    actions.add = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::TRASH,
                    "Delete selected",
                    archive_loaded && has_selection,
                ) {
                    actions.delete_selected = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::PACKAGE,
                    "Convert to 7z",
                    archive_loaded,
                ) {
                    actions.convert_to_7z = true;
                }
                if toolbar_button_with_text(
                    ui,
                    theme,
                    egui_phosphor::regular::FOLDERS,
                    "Organize",
                    archive_loaded,
                ) {
                    actions.organize_archive = true;
                }
            });
        });

        ui.add_space(4.0);

        // View mode group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                let list_selected = !state.grid_view;
                if toolbar_button_toggle(ui, theme, egui_phosphor::regular::LIST, list_selected) {
                    state.grid_view = false;
                }
                if toolbar_button_toggle(
                    ui,
                    theme,
                    egui_phosphor::regular::GRID_FOUR,
                    state.grid_view,
                ) {
                    state.grid_view = true;
                }
            });
        });

        ui.add_space(4.0);

        // Column resize toggle (only visible in list view)
        if !state.grid_view {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

                ui.horizontal_centered(|ui| {
                    let icon = if state.columns_locked {
                        egui_phosphor::regular::LOCK
                    } else {
                        egui_phosphor::regular::LOCK_OPEN
                    };
                    if toolbar_button_toggle(ui, theme, icon, state.columns_locked) {
                        state.columns_locked = !state.columns_locked;
                    }
                });
            });
        }

        // Panel toggles - right aligned
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

                ui.horizontal_centered(|ui| {
                    if toolbar_button_toggle(
                        ui,
                        theme,
                        egui_phosphor::regular::INFO,
                        state.show_properties_panel,
                    ) {
                        state.show_properties_panel = !state.show_properties_panel;
                    }
                    if toolbar_button_toggle(
                        ui,
                        theme,
                        egui_phosphor::regular::TREE_STRUCTURE,
                        state.show_tree_panel,
                    ) {
                        state.show_tree_panel = !state.show_tree_panel;
                    }
                });
            });
        });
    });

    actions
}

fn toolbar_button(ui: &mut egui::Ui, theme: &AppTheme, text: &str, enabled: bool) -> bool {
    let color = if enabled {
        theme.colors.on_surface
    } else {
        theme.colors.on_surface_variant
    };

    let button = egui::Button::new(egui::RichText::new(text).size(16.0).color(color))
        .fill(theme.colors.surface_variant)
        .stroke(egui::Stroke::NONE)
        .corner_radius(4.0)
        .min_size(egui::vec2(36.0, 32.0));

    ui.add_enabled(enabled, button).clicked()
}

fn toolbar_button_with_text(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    icon: &str,
    label: &str,
    enabled: bool,
) -> bool {
    let color = if enabled {
        theme.colors.on_surface
    } else {
        theme.colors.on_surface_variant
    };

    let button = egui::Button::new(
        egui::RichText::new(format!("{} {}", icon, label))
            .size(14.0)
            .color(color),
    )
    .fill(theme.colors.surface_variant)
    .stroke(egui::Stroke::NONE)
    .corner_radius(4.0)
    .min_size(egui::vec2(90.0, 32.0));

    ui.add_enabled(enabled, button).clicked()
}

fn toolbar_button_toggle(ui: &mut egui::Ui, theme: &AppTheme, text: &str, selected: bool) -> bool {
    let bg_fill = if selected {
        theme.colors.secondary
    } else {
        theme.colors.surface_variant
    };

    let button = egui::Button::new(
        egui::RichText::new(text)
            .size(16.0)
            .color(theme.colors.on_surface),
    )
    .fill(bg_fill)
    .stroke(egui::Stroke::NONE)
    .corner_radius(4.0)
    .min_size(egui::vec2(36.0, 32.0));

    ui.add(button).clicked()
}
