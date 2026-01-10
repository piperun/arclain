use super::types::{ButtonContext, ToolbarActions, ToolbarState};
use arclain_core::{ActionType, UiItem};
use arclain_plugins::types::{ButtonAction, PluginUiElement};
use arclain_theme::ButtonVariant;
use eframe::egui;
use egui::Widget;

/// Render a single toolbar button by ID, returns true if action triggered
pub fn render_button(
    ui: &mut egui::Ui,
    item: &UiItem,
    ctx: &ButtonContext,
    state: &mut ToolbarState,
    actions: &mut ToolbarActions,
) {
    if item.action_type == ActionType::Plugin {
        if let Some(action_data) = &item.action_data {
            // format: "plugin_id:button_id"
            if let Some((plugin_id, btn_id)) = action_data.split_once(':') {
                if let Some(elements) = ctx.plugin_elements.get(plugin_id) {
                    // Find button in cached elements
                    if let Some(PluginUiElement::Button {
                        id: _,
                        label,
                        action: btn_action,
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
                            let event_id = match btn_action.as_ref().unwrap_or(&ButtonAction::None)
                            {
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
                    use crate::features::plugins::ui::UiEventCallback;
                    let mut callback: UiEventCallback =
                        Box::new(move |element_id: &str, value: Option<String>| {
                            actions.plugin_events.push((
                                pid.clone(),
                                element_id.to_string(),
                                value,
                            ));
                        });

                    crate::features::plugins::ui::render_ui_elements(
                        ui,
                        elements,
                        &mut callback,
                        &ctx.theme.colors,
                        None,
                        ctx.shared,
                        Some(plugin_id.as_str()),
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
