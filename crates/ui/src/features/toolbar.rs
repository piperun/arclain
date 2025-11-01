use super::theme::AppTheme;
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

pub struct ToolbarActions {
    pub go_back: bool,
    pub go_forward: bool,
    pub go_up: bool,
    pub extract: bool,
    pub add: bool,
    pub open: bool,
    pub delete_selected: bool,
}

impl Default for ToolbarActions {
    fn default() -> Self {
        Self {
            go_back: false,
            go_forward: false,
            go_up: false,
            extract: false,
            add: false,
            open: false,
            delete_selected: false,
        }
    }
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
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Navigation group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if toolbar_button(ui, theme, "⬅", can_go_back) {
                    actions.go_back = true;
                }
                if toolbar_button(ui, theme, "➡", can_go_forward) {
                    actions.go_forward = true;
                }
                if toolbar_button(ui, theme, "⬆", can_go_up) {
                    actions.go_up = true;
                }
            });
        });

        ui.add_space(4.0);

        // File actions group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if toolbar_button_with_text(ui, theme, "📂", "Open", true) {
                    actions.open = true;
                }
                if toolbar_button_with_text(ui, theme, "⛏", "Extract", archive_loaded) {
                    actions.extract = true;
                }
                if toolbar_button_with_text(ui, theme, "➕", "Add", archive_loaded) {
                    actions.add = true;
                }
                if toolbar_button_with_text(ui, theme, "🗑", "Delete selected", archive_loaded && has_selection) {
                    actions.delete_selected = true;
                }
            });
        });

        ui.add_space(4.0);

        // View mode group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                let list_selected = !state.grid_view;
                if toolbar_button_toggle(ui, theme, "☰", list_selected) {
                    state.grid_view = false;
                }
                if toolbar_button_toggle(ui, theme, "▦", state.grid_view) {
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
                    let icon = if state.columns_locked { "🔒" } else { "🔓" };
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
                    if toolbar_button_toggle(ui, theme, "ℹ", state.show_properties_panel) {
                        state.show_properties_panel = !state.show_properties_panel;
                    }
                    if toolbar_button_toggle(ui, theme, "📂", state.show_tree_panel) {
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
        theme.colors.text_primary
    } else {
        theme.colors.text_muted
    };

    let button = egui::Button::new(egui::RichText::new(text).size(16.0).color(color))
        .fill(theme.colors.bg_tertiary)
        .stroke(egui::Stroke::NONE)
        .rounding(4.0)
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
        theme.colors.text_primary
    } else {
        theme.colors.text_muted
    };

    let button = egui::Button::new(
        egui::RichText::new(format!("{} {}", icon, label))
            .size(14.0)
            .color(color),
    )
    .fill(theme.colors.bg_tertiary)
    .stroke(egui::Stroke::NONE)
    .rounding(4.0)
    .min_size(egui::vec2(90.0, 32.0));

    ui.add_enabled(enabled, button).clicked()
}

fn toolbar_button_toggle(ui: &mut egui::Ui, theme: &AppTheme, text: &str, selected: bool) -> bool {
    let bg_fill = if selected {
        theme.colors.bg_hover
    } else {
        theme.colors.bg_tertiary
    };

    let button = egui::Button::new(
        egui::RichText::new(text)
            .size(16.0)
            .color(theme.colors.text_primary),
    )
    .fill(bg_fill)
    .stroke(egui::Stroke::NONE)
    .rounding(4.0)
    .min_size(egui::vec2(36.0, 32.0));

    ui.add(button).clicked()
}
