use crate::shared::theme::AppTheme;
use arclain_db::{DisplayMode, UiItem, UiRegion};
use arclain_theme::ButtonVariant;
use eframe::egui;
use egui::Widget;

/// Configuration for toolbar items loaded from database
pub struct ToolbarConfig {
    items: Vec<UiItem>,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl ToolbarConfig {
    pub fn new(items: Vec<UiItem>) -> Self {
        // Filter to only toolbar items and sort by sort_order
        let mut items: Vec<UiItem> = items
            .into_iter()
            .filter(|i| i.region == UiRegion::Toolbar)
            .collect();
        items.sort_by_key(|i| i.sort_order);
        Self { items }
    }

    /// Check if an item is visible by its id (e.g., "toolbar.back")
    pub fn is_visible(&self, id: &str) -> bool {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.visible)
            .unwrap_or(true) // Default to visible if not configured
    }

    /// Get display mode for an item
    pub fn display_mode(&self, id: &str) -> DisplayMode {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.display_mode)
            .unwrap_or(DisplayMode::IconAndText)
    }
}

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
    config: Option<&ToolbarConfig>,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    // Helper to check visibility
    let is_visible = |id: &str| config.map(|c| c.is_visible(id)).unwrap_or(true);

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Navigation group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if is_visible("toolbar.back") {
                    if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_LEFT)
                        .with_theme_colors(&theme.colors)
                        .variant(ButtonVariant::Ghost)
                        .enabled(can_go_back)
                        .ui(ui)
                        .clicked()
                    {
                        actions.go_back = true;
                    }
                }
                if is_visible("toolbar.forward") {
                    if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_RIGHT)
                        .with_theme_colors(&theme.colors)
                        .variant(ButtonVariant::Ghost)
                        .enabled(can_go_forward)
                        .ui(ui)
                        .clicked()
                    {
                        actions.go_forward = true;
                    }
                }
                if is_visible("toolbar.up") {
                    if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_UP)
                        .with_theme_colors(&theme.colors)
                        .variant(ButtonVariant::Ghost)
                        .enabled(can_go_up)
                        .ui(ui)
                        .clicked()
                    {
                        actions.go_up = true;
                    }
                }
            });
        });

        ui.add_space(4.0);

        // File actions group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if is_visible("toolbar.open") {
                    if arclain_widgets::TextButton::new(
                        format!("{} Open", egui_phosphor::regular::FOLDER_OPEN),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&theme.colors)
                    .variant(ButtonVariant::Ghost)
                    .ui(ui)
                    .clicked()
                    {
                        actions.open = true;
                    }
                }
                if is_visible("toolbar.extract") {
                    if ui
                        .add_enabled(
                            archive_loaded && has_selection,
                            arclain_widgets::TextButton::new(
                                format!("{} Extract", egui_phosphor::regular::EXPORT),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.extract = true;
                    }
                }
                if is_visible("toolbar.extract_all") {
                    if ui
                        .add_enabled(
                            archive_loaded,
                            arclain_widgets::TextButton::new(
                                format!("{} Extract all", egui_phosphor::regular::EXPORT),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.extract_all = true;
                    }
                }
                if is_visible("toolbar.add") {
                    if ui
                        .add_enabled(
                            archive_loaded,
                            arclain_widgets::TextButton::new(
                                format!("{} Add", egui_phosphor::regular::PLUS),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.add = true;
                    }
                }
                if is_visible("toolbar.delete") {
                    if ui
                        .add_enabled(
                            archive_loaded && has_selection,
                            arclain_widgets::TextButton::new(
                                format!("{} Delete selected", egui_phosphor::regular::TRASH),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.delete_selected = true;
                    }
                }
                if is_visible("toolbar.convert") {
                    if ui
                        .add_enabled(
                            archive_loaded,
                            arclain_widgets::TextButton::new(
                                format!("{} Convert to 7z", egui_phosphor::regular::PACKAGE),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.convert_to_7z = true;
                    }
                }
                if is_visible("toolbar.organize") {
                    if ui
                        .add_enabled(
                            archive_loaded,
                            arclain_widgets::TextButton::new(
                                format!("{} Organize", egui_phosphor::regular::FOLDERS),
                                arclain_widgets::ButtonSize::Custom {
                                    width: 90.0,
                                    height: 32.0,
                                },
                            )
                            .with_theme_colors(&theme.colors)
                            .variant(ButtonVariant::Ghost),
                        )
                        .clicked()
                    {
                        actions.organize_archive = true;
                    }
                }
            });
        });

        ui.add_space(4.0);

        // View mode group
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

            ui.horizontal_centered(|ui| {
                if is_visible("toolbar.list_view") {
                    let list_selected = !state.grid_view;
                    if arclain_widgets::ToggleButton::new(
                        egui_phosphor::regular::LIST,
                        list_selected,
                    )
                    .with_theme_colors(&theme.colors)
                    .ui(ui)
                    .clicked()
                    {
                        state.grid_view = false;
                    }
                }
                if is_visible("toolbar.grid_view") {
                    if arclain_widgets::ToggleButton::new(
                        egui_phosphor::regular::GRID_FOUR,
                        state.grid_view,
                    )
                    .with_theme_colors(&theme.colors)
                    .ui(ui)
                    .clicked()
                    {
                        state.grid_view = true;
                    }
                }
            });
        });

        ui.add_space(4.0);

        // Column resize toggle (only visible in list view)
        if !state.grid_view && is_visible("toolbar.column_lock") {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

                ui.horizontal_centered(|ui| {
                    let icon = if state.columns_locked {
                        egui_phosphor::regular::LOCK
                    } else {
                        egui_phosphor::regular::LOCK_OPEN
                    };
                    if arclain_widgets::ToggleButton::new(icon, state.columns_locked)
                        .with_theme_colors(&theme.colors)
                        .ui(ui)
                        .clicked()
                    {
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
                    if is_visible("toolbar.properties_panel") {
                        if arclain_widgets::ToggleButton::new(
                            egui_phosphor::regular::INFO,
                            state.show_properties_panel,
                        )
                        .with_theme_colors(&theme.colors)
                        .ui(ui)
                        .clicked()
                        {
                            state.show_properties_panel = !state.show_properties_panel;
                        }
                    }
                    if is_visible("toolbar.tree_panel") {
                        if arclain_widgets::ToggleButton::new(
                            egui_phosphor::regular::TREE_STRUCTURE,
                            state.show_tree_panel,
                        )
                        .with_theme_colors(&theme.colors)
                        .ui(ui)
                        .clicked()
                        {
                            state.show_tree_panel = !state.show_tree_panel;
                        }
                    }
                });
            });
        });
    });

    actions
}
