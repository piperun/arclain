//! Plugin rendering module
//!
//! Contains functions for rendering plugin dialogs and pages,
//! extracted from arclain_app.rs to keep feature logic self-contained.

use crate::shared::components::Form;
use crate::shared::SharedState;
use eframe::egui;
use std::sync::Arc;

/// Layout shown while we wait for the plugin instance lock to free up
/// (e.g. during a long-running DLSite fetch holding the lock on a
/// worker thread). Rendered only when there is no cached layout to
/// show in its place; if a previous layout exists we keep showing it
/// until the next refetch succeeds.
fn message_layout(message: impl Into<String>) -> Arc<arclain_plugins::types::PluginLayout> {
    use arclain_plugins::types::{PluginLayout, PluginUiElement};
    Arc::new(PluginLayout::Single {
        elements: vec![PluginUiElement::Label {
            text: message.into(),
            bold: false,
            size: None,
        }],
    })
}

fn loading_placeholder_layout() -> Arc<arclain_plugins::types::PluginLayout> {
    message_layout("Loading plugin UI…")
}

/// Return cached layout data or queue one worker request.
fn cached_or_request_layout(
    shared: &SharedState,
    plugin_id: &str,
    target: crate::features::plugins::application::PluginUiTarget,
    origin_tab: crate::core::tabs::TabId,
) -> Option<Result<Arc<arclain_plugins::types::PluginLayout>, Arc<str>>> {
    shared
        .plugin_ui_jobs
        .layout(plugin_id, target, Some(origin_tab))
}

/// Render an open plugin dialog as a modal overlay
pub fn render_dialog(ctx: &egui::Context, shared: &SharedState) {
    // Check if a dialog is open and get cached layout
    let (dialog_info, cached_layout) = {
        let dialog_state = shared.signals().plugin_dialog_state.get();
        let dialog_info = dialog_state.open_dialog.clone();
        let cached = dialog_state.cached_dialog_layout.clone();
        (dialog_info, cached)
    };

    if let Some((plugin_id, dialog_id, origin_tab)) = dialog_info {
        let target =
            crate::features::plugins::application::PluginUiTarget::Dialog(dialog_id.clone());
        let is_stale = shared
            .signals()
            .plugin_dialog_state
            .get()
            .cached_dialog_layout_stale;
        if is_stale {
            shared
                .plugin_ui_jobs
                .invalidate_layout(&plugin_id, &target, Some(origin_tab));
            let mut state = shared.signals().plugin_dialog_state.get();
            state.cached_dialog_layout_stale = false;
            shared.signals().plugin_dialog_state.set(state);
        }
        let dialog_elements = match cached_or_request_layout(shared, &plugin_id, target, origin_tab)
        {
            Some(Ok(fresh)) => {
                let mut state = shared.signals().plugin_dialog_state.get();
                state.cached_dialog_layout = Some(fresh.clone());
                shared.signals().plugin_dialog_state.set(state);
                fresh
            }
            Some(Err(error)) => message_layout(format!("Plugin UI error: {error}")),
            None => cached_layout.unwrap_or_else(loading_placeholder_layout),
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


                let mut render = |elements: &[arclain_plugins::types::PluginUiElement]| {
                    super::ui::render_ui_elements(
                        ui,
                        elements,
                        &mut callback,
                        &shared.theme.colors,
                        None,
                        Some(shared),
                        Some(&plugin_id),
                    );
                };
                match dialog_elements.as_ref() {
                    arclain_plugins::types::PluginLayout::Single { elements } => render(elements),
                    arclain_plugins::types::PluginLayout::Split {
                        sidebar, content, ..
                    } => {
                        render(sidebar);
                        render(content);
                    }
                }
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
            .map(|(plugin, page, origin_tab)| (plugin.to_string(), page.to_string(), origin_tab));
        let cached = dialog_state.cached_page_layout.clone();
        (page_info, cached)
    };

    let Some((plugin_id, page_id, origin_tab)) = page_info else {
        return false;
    };

    // Queue __page_init once for this page generation. The worker
    // captures the origin tab; stale generations are ignored when
    // results are applied before the next render.
    if let Some((request_id, pending_plugin, pending_page, origin_tab)) = shared
        .signals()
        .plugin_dialog_state
        .get()
        .pending_page_init()
        .map(|(id, plugin, page, origin_tab)| {
            (id, plugin.to_string(), page.to_string(), origin_tab)
        })
    {
        shared.plugin_ui_jobs.request_with_id(
            request_id,
            crate::features::plugins::application::PluginUiRequest::PageInit {
                plugin_id: pending_plugin,
                page_id: pending_page,
                origin_tab,
            },
        );
    }

    // Do not cache a pre-init layout. Once the matching initialization
    // result is applied it invalidates this exact page key and the next
    // frame queues the first valid layout read.
    let page_layout_ready = shared
        .signals()
        .plugin_dialog_state
        .get()
        .page_layout_ready();
    let is_stale = shared
        .signals()
        .plugin_dialog_state
        .get()
        .cached_page_layout_stale;
    let target = crate::features::plugins::application::PluginUiTarget::Page(page_id.clone());
    if page_layout_ready && is_stale {
        shared
            .plugin_ui_jobs
            .invalidate_layout(&plugin_id, &target, Some(origin_tab));
        let mut state = shared.signals().plugin_dialog_state.get();
        state.cached_page_layout_stale = false;
        shared.signals().plugin_dialog_state.set(state);
    }
    let page_layout = if page_layout_ready {
        match cached_or_request_layout(shared, &plugin_id, target, origin_tab) {
            Some(Ok(fresh)) => {
                let mut state = shared.signals().plugin_dialog_state.get();
                state.cached_page_layout = Some(fresh.clone());
                shared.signals().plugin_dialog_state.set(state);
                fresh
            }
            Some(Err(error)) => message_layout(format!("Plugin UI error: {error}")),
            None => cached_layout.unwrap_or_else(loading_placeholder_layout),
        }
    } else {
        cached_layout.unwrap_or_else(loading_placeholder_layout)
    };

    // Get display name from signal, fallback to page_id
    let display_name = shared
        .signals()
        .tabs
        .get()
        .get(origin_tab)
        .and_then(|tab| tab.page_display_name.get())
        .unwrap_or_else(|| page_id.clone());

    // Render as full page content
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(shared.theme.colors.surface))
        .show(ctx, |ui| {
            // Page title (improved styling, stays visible)
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new(&display_name)
                        .strong()
                        .size(16.0)
                        .color(shared.theme.colors.on_surface),
                );
            });
            ui.add_space(4.0);
            ui.separator();

            // Set up callback for page events
            let mut callback = crate::features::plugins::presentation::controllers::plugin_controller::create_page_callback(shared, plugin_id.clone(), origin_tab);


            use arclain_plugins::types::PluginLayout;
            let content_cache = shared.services.content_cache.clone();

            match page_layout.as_ref() {
                PluginLayout::Single { elements } => {
                    // Wrap in Form to provide ScrollArea (fixes cutoff bug)
                    Form::new()
                        .id(format!("plugin_page_single_{}", page_id))
                        .margin(16.0)
                        .show(ui, &shared.theme, |ui| {
                            super::ui::render_ui_elements(
                                ui,
                                &elements,
                                &mut callback,
                                &shared.theme.colors,
                                content_cache.as_ref(),
                                Some(shared),
                                Some(&plugin_id),
                            );
                        });
                }
                PluginLayout::Split {
                    sidebar,
                    content,
                    sidebar_width,
                } => {
                    // Wrap in Frame for consistent margin
                    egui::Frame::NONE
                        .inner_margin(16.0)
                        .show(ui, |ui| {
                            egui::SidePanel::left(format!("plugin_split_sidebar_{}", page_id))
                                .resizable(true)
                                .default_width(sidebar_width.unwrap_or(250.0))
                                .show_inside(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt(format!("plugin_split_sidebar_scroll_{}", page_id))
                                        .show(ui, |ui| {
                                            super::ui::render_ui_elements(
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
                                egui::ScrollArea::vertical()
                                    .id_salt(format!("plugin_split_content_scroll_{}", page_id))
                                    .show(ui, |ui| {
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
                        });
                }
            }
        });

    true
}
