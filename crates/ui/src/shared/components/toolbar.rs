use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_db::{DisplayMode, UiItem, UiRegion};
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::PluginExtensionPoint;
use arclain_theme::ButtonVariant;
use eframe::egui;
use egui::Widget;
use parking_lot::Mutex;
use std::sync::Arc;

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
    #[allow(dead_code)]
    pub fn is_visible(&self, id: &str) -> bool {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.visible)
            .unwrap_or(true) // Default to visible if not configured
    }

    /// Get display mode for an item
    #[allow(dead_code)]
    pub fn display_mode(&self, id: &str) -> DisplayMode {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.display_mode)
            .unwrap_or(DisplayMode::IconAndText)
    }

    /// Get visible items grouped by group_id, in sort order
    pub fn items_by_group(&self) -> Vec<(Option<String>, Vec<&UiItem>)> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<Option<String>, Vec<&UiItem>> = BTreeMap::new();

        for item in self.items.iter().filter(|i| i.visible) {
            groups.entry(item.group_id.clone()).or_default().push(item);
        }

        // Convert to vec, maintaining group order by first item's sort_order
        let mut result: Vec<_> = groups.into_iter().collect();
        result.sort_by_key(|(_, items)| items.first().map(|i| i.sort_order).unwrap_or(0));
        result
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

/// Context for button rendering
struct ButtonContext<'a> {
    theme: &'a AppTheme,
    can_go_back: bool,
    can_go_forward: bool,
    can_go_up: bool,
    archive_loaded: bool,
    has_selection: bool,
}

/// Render a single toolbar button by ID, returns true if action triggered
fn render_button(
    ui: &mut egui::Ui,
    item: &UiItem,
    ctx: &ButtonContext,
    state: &mut ToolbarState,
    actions: &mut ToolbarActions,
) {
    match item.id.as_str() {
        // Navigation buttons (icon only)
        "toolbar.back" => {
            if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_LEFT)
                .with_theme_colors(&ctx.theme.colors)
                .variant(ButtonVariant::Ghost)
                .enabled(ctx.can_go_back)
                .ui(ui)
                .clicked()
            {
                actions.go_back = true;
            }
        }
        "toolbar.forward" => {
            if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_RIGHT)
                .with_theme_colors(&ctx.theme.colors)
                .variant(ButtonVariant::Ghost)
                .enabled(ctx.can_go_forward)
                .ui(ui)
                .clicked()
            {
                actions.go_forward = true;
            }
        }
        "toolbar.up" => {
            if arclain_widgets::IconButton::new(egui_phosphor::regular::ARROW_UP)
                .with_theme_colors(&ctx.theme.colors)
                .variant(ButtonVariant::Ghost)
                .enabled(ctx.can_go_up)
                .ui(ui)
                .clicked()
            {
                actions.go_up = true;
            }
        }
        // File action buttons (text + icon)
        "toolbar.open" => {
            if arclain_widgets::TextButton::new(
                format!("{} Open", egui_phosphor::regular::FOLDER_OPEN),
                arclain_widgets::ButtonSize::Custom {
                    width: 90.0,
                    height: 32.0,
                },
            )
            .with_theme_colors(&ctx.theme.colors)
            .variant(ButtonVariant::Ghost)
            .ui(ui)
            .clicked()
            {
                actions.open = true;
            }
        }
        "toolbar.extract" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded && ctx.has_selection,
                    arclain_widgets::TextButton::new(
                        format!("{} Extract", egui_phosphor::regular::EXPORT),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.extract = true;
            }
        }
        "toolbar.extract_all" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded,
                    arclain_widgets::TextButton::new(
                        format!("{} Extract all", egui_phosphor::regular::EXPORT),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.extract_all = true;
            }
        }
        "toolbar.add" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded,
                    arclain_widgets::TextButton::new(
                        format!("{} Add", egui_phosphor::regular::PLUS),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.add = true;
            }
        }
        "toolbar.delete" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded && ctx.has_selection,
                    arclain_widgets::TextButton::new(
                        format!("{} Delete", egui_phosphor::regular::TRASH),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.delete_selected = true;
            }
        }
        "toolbar.convert" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded,
                    arclain_widgets::TextButton::new(
                        format!("{} Convert to 7z", egui_phosphor::regular::PACKAGE),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.convert_to_7z = true;
            }
        }
        "toolbar.organize" => {
            if ui
                .add_enabled(
                    ctx.archive_loaded,
                    arclain_widgets::TextButton::new(
                        format!("{} Organize", egui_phosphor::regular::FOLDERS),
                        arclain_widgets::ButtonSize::Custom {
                            width: 90.0,
                            height: 32.0,
                        },
                    )
                    .with_theme_colors(&ctx.theme.colors)
                    .variant(ButtonVariant::Ghost),
                )
                .clicked()
            {
                actions.organize_archive = true;
            }
        }
        // View mode buttons (toggle)
        "toolbar.list_view" => {
            let list_selected = !state.grid_view;
            if arclain_widgets::ToggleButton::new(egui_phosphor::regular::LIST, list_selected)
                .with_theme_colors(&ctx.theme.colors)
                .ui(ui)
                .clicked()
            {
                state.grid_view = false;
            }
        }
        "toolbar.grid_view" => {
            if arclain_widgets::ToggleButton::new(
                egui_phosphor::regular::GRID_FOUR,
                state.grid_view,
            )
            .with_theme_colors(&ctx.theme.colors)
            .ui(ui)
            .clicked()
            {
                state.grid_view = true;
            }
        }
        "toolbar.column_lock" => {
            // Only show in list view
            if !state.grid_view {
                let icon = if state.columns_locked {
                    egui_phosphor::regular::LOCK
                } else {
                    egui_phosphor::regular::LOCK_OPEN
                };
                if arclain_widgets::ToggleButton::new(icon, state.columns_locked)
                    .with_theme_colors(&ctx.theme.colors)
                    .ui(ui)
                    .clicked()
                {
                    state.columns_locked = !state.columns_locked;
                }
            }
        }
        // Panel toggles
        "toolbar.tree_panel" => {
            if arclain_widgets::ToggleButton::new(
                egui_phosphor::regular::TREE_STRUCTURE,
                state.show_tree_panel,
            )
            .with_theme_colors(&ctx.theme.colors)
            .ui(ui)
            .clicked()
            {
                state.show_tree_panel = !state.show_tree_panel;
            }
        }
        "toolbar.properties_panel" => {
            if arclain_widgets::ToggleButton::new(
                egui_phosphor::regular::INFO,
                state.show_properties_panel,
            )
            .with_theme_colors(&ctx.theme.colors)
            .ui(ui)
            .clicked()
            {
                state.show_properties_panel = !state.show_properties_panel;
            }
        }
        _ => {
            // Unknown button - skip or log
            tracing::debug!("Unknown toolbar item: {}", item.id);
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
    _has_metadata: bool,
    config: Option<&ToolbarConfig>,
    plugin_manager: Option<&Arc<Mutex<PluginManager>>>,
    shared: Option<&SharedState>,
) -> ToolbarActions {
    let mut actions = ToolbarActions::default();

    // Collect plugin actions for processing after render
    let collected_actions: Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Collect dialog signals for processing after render
    let dialog_signals: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));

    let ctx = ButtonContext {
        theme,
        can_go_back,
        can_go_forward,
        can_go_up,
        archive_loaded,
        has_selection,
    };

    // If no config, render nothing (or could have a fallback)
    let Some(config) = config else {
        return actions;
    };

    let groups = config.items_by_group();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

        // Track which groups we've rendered (for right-aligned panels)
        let mut rendered_panels = false;

        for (group_id, items) in &groups {
            // Panel toggles go to the right side
            if group_id.as_deref() == Some("panels") {
                rendered_panels = true;
                continue; // Render later
            }

            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                ui.horizontal_centered(|ui| {
                    for item in items {
                        render_button(ui, item, &ctx, state, &mut actions);
                    }
                });
            });

            ui.add_space(4.0);
        }

        // Render plugin toolbar UI elements
        if let Some(manager_arc) = plugin_manager {
            let manager = manager_arc.lock();
            let plugins: Vec<String> = manager
                .list_plugins()
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.id.clone())
                .collect();

            for plugin_id in plugins {
                let pid = plugin_id.clone();
                let actions_sink = collected_actions.clone();
                let _ = manager.with_plugin_instance(&plugin_id, |instance| {
                    if let Ok(ui_elements) =
                        instance.get_ui_layout(PluginExtensionPoint::PluginButton)
                    {
                        if !ui_elements.is_empty() {
                            ui.separator();
                            ui.scope(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
                                let pid_inner = pid.clone();
                                let sink = actions_sink.clone();
                                let dialog_sink = dialog_signals.clone();
                                let mut callback: Box<dyn FnMut(&str, Option<String>)> =
                                    Box::new(move |element_id: &str, value: Option<String>| {
                                        // Check for dialog control signals
                                        if element_id.starts_with("__dialog_open:") {
                                            let dialog_id = element_id
                                                .trim_start_matches("__dialog_open:")
                                                .to_string();
                                            dialog_sink.lock().push((pid_inner.clone(), dialog_id));
                                            return;
                                        }
                                        if element_id == "__dialog_close" {
                                            dialog_sink
                                                .lock()
                                                .push((pid_inner.clone(), "__close".to_string()));
                                            return;
                                        }

                                        // Normal event - send to plugin
                                        if let Some(actions) =
                                            instance.send_ui_event(element_id, value).ok()
                                        {
                                            let mut s = sink.lock();
                                            for a in actions {
                                                s.push((pid_inner.clone(), a));
                                            }
                                        }
                                    });
                                crate::features::plugins::plugin_ui::render_ui_elements(
                                    ui,
                                    &ui_elements,
                                    &mut callback,
                                    &theme.colors,
                                    None, // TODO: wire content_cache through
                                );
                            });
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                });
            }
        }

        // Panel toggles - right aligned
        if rendered_panels {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                    ui.horizontal_centered(|ui| {
                        // Find panels group and render in reverse (for right-to-left)
                        for (group_id, items) in groups.iter().rev() {
                            if group_id.as_deref() == Some("panels") {
                                for item in items.iter().rev() {
                                    render_button(ui, item, &ctx, state, &mut actions);
                                }
                            }
                        }
                    });
                });
            });
        }
    });

    // Process collected plugin actions and dialog signals
    if let Some(shared) = shared {
        let actions_list = collected_actions.lock();
        let mut toaster = shared.toaster.lock();
        let mut dialog_state = shared.plugin_dialog_state.lock();

        for (plugin_id, plugin_action) in actions_list.iter() {
            crate::features::plugins::action_handler::process_plugin_actions(
                vec![plugin_action.clone()],
                plugin_id,
                &mut dialog_state,
                &mut toaster,
                Some(&shared.refresh_requests),
            );
        }

        // Process dialog signals
        let dialog_sigs = dialog_signals.lock();
        for (plugin_id, dialog_id) in dialog_sigs.iter() {
            if dialog_id == "__close" {
                dialog_state.close_dialog();
            } else {
                dialog_state.open_dialog(plugin_id, dialog_id);
            }
        }
    }

    actions
}
