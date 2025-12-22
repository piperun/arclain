use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_db::{DisplayMode, UiItem, UiRegion};
use arclain_plugins::manager::PluginManager;
use arclain_plugins::types::{ButtonAction, PluginExtensionPoint, PluginUiElement};
use arclain_theme::ButtonVariant;
use eframe::egui;
use egui::Widget;
use parking_lot::Mutex;
use std::collections::HashMap;
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
    /// Collected plugin events: (plugin_id, element_id, value)
    pub plugin_events: Vec<(String, String, Option<String>)>,
}

/// Context for button rendering
struct ButtonContext<'a> {
    theme: &'a AppTheme,
    can_go_back: bool,
    can_go_forward: bool,
    can_go_up: bool,
    archive_loaded: bool,
    has_selection: bool,
    /// Cached plugin UI elements by plugin_id
    plugin_elements: HashMap<String, Vec<PluginUiElement>>,
}

/// Render a single toolbar button by ID, returns true if action triggered
fn render_button(
    ui: &mut egui::Ui,
    item: &UiItem,
    ctx: &ButtonContext,
    state: &mut ToolbarState,
    actions: &mut ToolbarActions,
) {
    if item.action_type == arclain_db::ActionType::Plugin {
        if let Some(action_data) = &item.action_data {
            // format: "plugin_id:button_id"
            if let Some((plugin_id, btn_id)) = action_data.split_once(':') {
                if let Some(elements) = ctx.plugin_elements.get(plugin_id) {
                    // Find button in cached elements
                    if let Some(PluginUiElement::Button {
                        id: _,
                        label,
                        action,
                    }) = elements
                        .iter()
                        .find(|e| matches!(e, PluginUiElement::Button { id, .. } if id == btn_id))
                    {
                        // Render button
                        if arclain_widgets::TextButton::new(
                            label,
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
                            // Handle action
                            let event_id = match action.as_ref().unwrap_or(&ButtonAction::None) {
                                ButtonAction::ShowDialog { id } => format!("__dialog_open:{}", id),
                                ButtonAction::CloseDialog => "__dialog_close".to_string(),
                                ButtonAction::OpenPage { id } => format!("__page_open:{}", id),
                                ButtonAction::ClosePage => "__page_close".to_string(),
                                ButtonAction::Custom(custom_id) => custom_id.clone(),
                                ButtonAction::None => btn_id.to_string(),
                            };

                            actions
                                .plugin_events
                                .push((plugin_id.to_string(), event_id, None));
                        }
                    }
                }
            } else {
                // Legacy: render all buttons for plugin
                let plugin_id = action_data;
                if let Some(elements) = ctx.plugin_elements.get(plugin_id) {
                    let pid = plugin_id.clone();
                    use crate::features::plugins::plugin_ui::UiEventCallback;
                    let mut callback: UiEventCallback =
                        Box::new(move |element_id: &str, value: Option<String>| {
                            actions.plugin_events.push((
                                pid.clone(),
                                element_id.to_string(),
                                value,
                            ));
                        });

                    crate::features::plugins::plugin_ui::render_ui_elements(
                        ui,
                        elements,
                        &mut callback,
                        &ctx.theme.colors,
                        None,
                    );
                }
            }
        }
        return;
    }

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

    // Pre-fetch plugin elements
    let mut plugin_elements = HashMap::new();
    if let Some(manager_arc) = plugin_manager {
        let manager = manager_arc.lock();
        let plugins = manager.list_plugins();
        for plugin in plugins.iter().filter(|p| p.enabled) {
            let pid = plugin.id.clone();
            let _ = manager.with_plugin_instance(&pid, |instance| {
                if let Ok(layout) = instance.get_ui_layout(PluginExtensionPoint::PluginButton) {
                    plugin_elements.insert(pid.clone(), layout.flatten());
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }

    let ctx = ButtonContext {
        theme,
        can_go_back,
        can_go_forward,
        can_go_up,
        archive_loaded,
        has_selection,
        plugin_elements,
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

        // Legacy plugin rendering removed (now handled via standard items)

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

    // Process plugin events collected from render_button
    if let Some(manager_arc) = plugin_manager {
        let manager = manager_arc.lock();
        let actions_sink = collected_actions.clone();
        let dialog_sink = dialog_signals.clone();

        for (plugin_id, event_id, value) in actions.plugin_events.drain(..) {
            // Check for dialog control signals
            if event_id.starts_with("__dialog_open:") {
                let dialog_id = event_id.trim_start_matches("__dialog_open:").to_string();
                dialog_sink.lock().push((plugin_id.clone(), dialog_id));
                continue;
            }
            if event_id == "__dialog_close" {
                dialog_sink
                    .lock()
                    .push((plugin_id.clone(), "__close".to_string()));
                continue;
            }

            // Send to plugin
            let _ = manager.with_plugin_instance(&plugin_id, |instance| {
                if let Ok(returned_actions) = instance.send_ui_event(&event_id, value.clone()) {
                    let mut sink = actions_sink.lock();
                    for a in returned_actions {
                        sink.push((plugin_id.clone(), a));
                    }
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }

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
