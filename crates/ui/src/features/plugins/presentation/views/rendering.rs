//! Plugin rendering module
//!
//! Contains functions for rendering plugin dialogs and pages,
//! extracted from arclain_app.rs to keep feature logic self-contained.

use crate::shared::SharedState;
use eframe::egui;

/// Render an open plugin dialog as a modal overlay
pub fn render_dialog(ctx: &egui::Context, shared: &SharedState) {
    // Check if a dialog is open and get cached layout
    let (dialog_info, cached_layout) = {
        let dialog_state = shared.signals().plugin_dialog_state.get();
        let dialog_info = dialog_state.open_dialog.clone();
        let cached = dialog_state.cached_dialog_layout.clone();
        (dialog_info, cached)
    };

    if let Some((plugin_id, dialog_id)) = dialog_info {
        // Use cached layout if available, otherwise fetch from plugin
        let dialog_elements = if let Some(layout) = cached_layout {
            layout
        } else {
            // Fetch layout from plugin (only on first render or after invalidation)
            let layout = {
                if let Some(pm_arc) = &shared.services.plugin_manager {
                    let pm = pm_arc.lock();
                    pm.with_plugin_instance(&plugin_id, |instance| {
                        instance
                            .get_ui_layout(arclain_plugins::types::PluginExtensionPoint::Dialog(
                                dialog_id.clone(),
                            ))
                            .unwrap_or_default()
                    })
                    .unwrap_or_default()
                } else {
                    arclain_plugins::types::PluginLayout::default()
                }
            };
            // Store in cache for next frame
            let mut dialog_state = shared.signals().plugin_dialog_state.get();
            dialog_state.cached_dialog_layout = Some(layout.clone());
            shared.signals().plugin_dialog_state.set(dialog_state);
            layout
        };

        // Render modal dialog
        let mut open = true;
        egui::Window::new(format!("Plugin Dialog - {}", dialog_id))
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([400.0, 300.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let mut callback =
                    crate::features::plugins::presentation::controllers::plugin_controller::create_dialog_callback(shared, plugin_id.clone());


                let flat_elements = dialog_elements.flatten();
                super::ui::render_ui_elements(
                    ui,
                    &flat_elements,
                    &mut callback,
                    &shared.theme.colors,
                    None,
                    Some(shared),
                    Some(&plugin_id),
                );
            });

        // If window was closed via X button
        if !open {
            let mut dialog_state = shared.signals().plugin_dialog_state.get();
            dialog_state.close_dialog();
            shared.signals().plugin_dialog_state.set(dialog_state);
        }
    }
}

/// Render an open plugin page (replaces main content area)
/// Returns true if a page is being rendered (caller should skip normal content)
pub fn render_page(ctx: &egui::Context, shared: &SharedState) -> bool {
    // Check if a page is open and get cached layout
    let (page_info, cached_layout) = {
        let dialog_state = shared.signals().plugin_dialog_state.get();
        let page_info = dialog_state
            .current_page()
            .map(|(p, d)| (p.to_string(), d.to_string()));
        let cached = dialog_state.cached_page_layout.clone();
        (page_info, cached)
    };

    let Some((plugin_id, page_id)) = page_info else {
        return false;
    };

    // Use cached layout if available, otherwise fetch from plugin
    let page_layout = if let Some(layout) = cached_layout {
        layout
    } else {
        // Fetch layout from plugin (only on first render or after invalidation)
        let layout = {
            if let Some(pm_arc) = &shared.services.plugin_manager {
                let pm = pm_arc.lock();
                pm.with_plugin_instance(&plugin_id, |instance| {
                    instance
                        .get_ui_layout(arclain_plugins::types::PluginExtensionPoint::Page(
                            page_id.clone(),
                        ))
                        .unwrap_or_default()
                })
                .unwrap_or_default()
            } else {
                arclain_plugins::types::PluginLayout::default()
            }
        };
        // Store in cache for next frame
        let mut dialog_state = shared.signals().plugin_dialog_state.get();
        dialog_state.cached_page_layout = Some(layout.clone());
        shared.signals().plugin_dialog_state.set(dialog_state);
        layout
    };

    // Render as full page content
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface))
        .show(ctx, |ui| {
            // Page title (no back button - use tab navigation)
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&page_id).strong());
            });
            ui.separator();

            // Set up callback for page events
            let mut callback = crate::features::plugins::presentation::controllers::plugin_controller::create_page_callback(shared, plugin_id.clone());


            use arclain_plugins::types::PluginLayout;
            let content_cache = shared.services.content_cache.clone();
            match page_layout {
                PluginLayout::Single { elements } => {
                    super::ui::render_ui_elements(
                        ui,
                        &elements,
                        &mut callback,
                        &shared.theme.colors,
                        content_cache.as_ref(),
                        Some(shared),
                        Some(&plugin_id),
                    );
                }
                PluginLayout::Split {
                    sidebar,
                    content,
                    sidebar_width,
                } => {
                    egui::SidePanel::left(format!("plugin_split_sidebar_{}", page_id))
                        .resizable(true)
                        .default_width(sidebar_width.unwrap_or(250.0))
                        .show_inside(ui, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                crate::features::plugins::presentation::rendering::render_ui_elements(

                                    ui,
                                    &sidebar,
                                    &mut callback,
                                    &shared.theme.colors,
                                    content_cache.as_ref(),
                                    Some(shared),
                                    Some(&plugin_id),
                                );
                            });
                        });

                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            super::ui::render_ui_elements(
                                ui,
                                &content,
                                &mut callback,
                                &shared.theme.colors,
                                content_cache.as_ref(),
                                Some(shared),
                                Some(&plugin_id),
                            );
                        });
                    });
                }
            }
        });

    true
}
