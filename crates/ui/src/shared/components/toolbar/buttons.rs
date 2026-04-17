use super::types::{ButtonContext, ToolbarActions, ToolbarState};
use arclain_core::{ActionType, UiItem};
use arclain_plugins::types::PluginUiElement;
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
                        action: _,
                    }) = elements
                        .iter()
                        .find(|e| matches!(e, PluginUiElement::Button { id, .. } if id == btn_id))
                    {
                        // Plugin buttons keep text (they're unique/unfamiliar)
                        if arclain_widgets::TextButton::new(
                            label,
                            arclain_widgets::ButtonSize::Small,
                        )
                        .with_theme_colors(&ctx.theme.colors)
                        .variant(ButtonVariant::Ghost)
                        .ui(ui)
                        .clicked()
                        {
                            actions.plugin_events.push((
                                plugin_id.to_string(),
                                btn_id.to_string(),
                                None,
                            ));
                        }
                    }
                }
            } else {
                // Legacy: render all buttons for plugin
                let plugin_id = action_data;
                if let Some(elements) = ctx.plugin_elements.get(plugin_id) {
                    let pid = plugin_id.clone();
                    use crate::features::plugins::presentation::rendering::UiEventCallback;

                    let mut callback: UiEventCallback =
                        Box::new(move |element_id: &str, value: Option<String>| {
                            actions.plugin_events.push((
                                pid.clone(),
                                element_id.to_string(),
                                value,
                            ));
                        });

                    crate::features::plugins::presentation::rendering::render_ui_elements(
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
        // ── Navigation (always icon-only) ──────────────────────
        "toolbar.back" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::ARROW_LEFT, "Back", ctx.can_go_back, || actions.go_back = true);
        }
        "toolbar.forward" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::ARROW_RIGHT, "Forward", ctx.can_go_forward, || actions.go_forward = true);
        }
        "toolbar.up" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::ARROW_UP, "Up one level", ctx.can_go_up, || actions.go_up = true);
        }

        // ── File operations ─────────────────────────────────────
        "toolbar.open" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::FOLDER_OPEN, "Open", true, || actions.open = true);
        }
        "toolbar.extract" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::EXPORT, "Extract", ctx.archive_loaded && ctx.has_selection, || actions.extract = true);
        }
        "toolbar.extract_all" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::TRAY_ARROW_DOWN, "Extract all", ctx.archive_loaded, || actions.extract_all = true);
        }
        "toolbar.add" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::PLUS, "Add", ctx.archive_loaded, || actions.add = true);
        }
        "toolbar.delete" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::TRASH, "Delete", ctx.archive_loaded && ctx.has_selection, || actions.delete_selected = true);
        }

        // ── Conversion / Organization ───────────────────────────
        "toolbar.convert" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::PACKAGE, "Convert...", ctx.archive_loaded, || actions.convert_to_7z = true);
        }
        "toolbar.batch_convert" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::FOLDER_PLUS, "Batch Convert...", true, || actions.batch_convert = true);
        }
        "toolbar.organize" => {
            toolbar_button(ui, ctx, egui_phosphor::regular::FOLDERS, "Organize", ctx.archive_loaded, || actions.organize_archive = true);
        }

        // ── View mode toggles ───────────────────────────────────
        "toolbar.list_view" => {
            let selected = !state.grid_view;
            if arclain_widgets::ToggleButton::new(egui_phosphor::regular::LIST, selected)
                .with_theme_colors(&ctx.theme.colors)
                .ui(ui)
                .on_hover_text("List view")
                .clicked()
            {
                state.grid_view = false;
            }
        }
        "toolbar.grid_view" => {
            if arclain_widgets::ToggleButton::new(egui_phosphor::regular::GRID_FOUR, state.grid_view)
                .with_theme_colors(&ctx.theme.colors)
                .ui(ui)
                .on_hover_text("Grid view")
                .clicked()
            {
                state.grid_view = true;
            }
        }
        "toolbar.column_lock" => {
            if !state.grid_view {
                let icon = if state.columns_locked {
                    egui_phosphor::regular::LOCK
                } else {
                    egui_phosphor::regular::LOCK_OPEN
                };
                if arclain_widgets::ToggleButton::new(icon, state.columns_locked)
                    .with_theme_colors(&ctx.theme.colors)
                    .ui(ui)
                    .on_hover_text(if state.columns_locked { "Unlock columns" } else { "Lock columns" })
                    .clicked()
                {
                    state.columns_locked = !state.columns_locked;
                }
            }
        }

        // ── Panel toggles ───────────────────────────────────────
        "toolbar.tree_panel" => {
            if arclain_widgets::ToggleButton::new(egui_phosphor::regular::TREE_STRUCTURE, state.show_tree_panel)
                .with_theme_colors(&ctx.theme.colors)
                .ui(ui)
                .on_hover_text("Tree panel")
                .clicked()
            {
                state.show_tree_panel = !state.show_tree_panel;
            }
        }
        "toolbar.properties_panel" => {
            if arclain_widgets::ToggleButton::new(egui_phosphor::regular::INFO, state.show_properties_panel)
                .with_theme_colors(&ctx.theme.colors)
                .ui(ui)
                .on_hover_text("Properties panel")
                .clicked()
            {
                state.show_properties_panel = !state.show_properties_panel;
            }
        }
        _ => {
            tracing::debug!("Unknown toolbar item: {}", item.id);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Toolbar button: icon-only when labels are off, icon+text when on.
fn toolbar_button(
    ui: &mut egui::Ui,
    ctx: &ButtonContext,
    icon: &str,
    label: &str,
    enabled: bool,
    mut on_click: impl FnMut(),
) {
    let response = if ctx.show_labels {
        ui.add_enabled(
            enabled,
            arclain_widgets::TextButton::new(
                format!("{}  {}", icon, label),
                arclain_widgets::ButtonSize::Small,
            )
            .with_theme_colors(&ctx.theme.colors)
            .variant(ButtonVariant::Ghost),
        )
    } else {
        ui.add_enabled(
            enabled,
            arclain_widgets::IconButton::new(icon)
                .with_theme_colors(&ctx.theme.colors)
                .variant(ButtonVariant::Ghost),
        )
    };
    if response.on_hover_text(label).clicked() {
        on_click();
    }
}
