//! Plugin rendering module
//!
//! Contains functions for rendering plugin dialogs and pages,
//! extracted from arclain_app.rs to keep feature logic self-contained.

use crate::shared::components::Form;
use crate::shared::SharedState;
use eframe::egui;

/// Layout shown while we wait for the plugin instance lock to free up
/// (e.g. during a long-running DLSite fetch holding the lock on a
/// worker thread). Rendered only when there is no cached layout to
/// show in its place; if a previous layout exists we keep showing it
/// until the next refetch succeeds.
fn loading_placeholder_layout() -> arclain_plugins::types::PluginLayout {
    use arclain_plugins::types::{PluginLayout, PluginUiElement};
    PluginLayout::Single {
        elements: vec![PluginUiElement::Label {
            text: "Loading plugin UI…".to_string(),
            bold: false,
            size: None,
        }],
    }
}

/// Try to fetch the layout for a plugin extension point without
/// blocking the UI thread. Returns the layout if the lock was free,
/// `None` if a worker thread is mid-event and we should keep using
/// whatever the caller has cached.
fn try_fetch_layout(
    shared: &SharedState,
    plugin_id: &str,
    point: arclain_plugins::types::PluginExtensionPoint,
) -> Option<arclain_plugins::types::PluginLayout> {
    let pm_arc = shared.services.plugin_manager.as_ref()?;
    let pm = pm_arc.lock();
    pm.try_with_plugin_instance(plugin_id, |instance| {
        instance.get_ui_layout(point).unwrap_or_default()
    })
    .flatten()
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

    if let Some((plugin_id, dialog_id)) = dialog_info {
        // Resolve which layout to render this frame:
        //   - cache + not stale → use cache directly (cheap path)
        //   - cache + stale     → try refetch; on success replace
        //                         cache, on busy keep stale visible
        //                         so the user doesn't see a blank
        //                         dialog while the worker is mid-event
        //   - no cache          → try fetch; on success cache it,
        //                         on busy show "Loading…" placeholder
        let is_stale = shared.signals().plugin_dialog_state.get().cached_dialog_layout_stale;
        let dialog_elements = match (cached_layout, is_stale) {
            (Some(layout), false) => layout,
            (Some(stale), true) => {
                if let Some(fresh) = try_fetch_layout(
                    shared,
                    &plugin_id,
                    arclain_plugins::types::PluginExtensionPoint::Dialog(dialog_id.clone()),
                ) {
                    let mut ds = shared.signals().plugin_dialog_state.get();
                    ds.cached_dialog_layout = Some(fresh.clone());
                    ds.cached_dialog_layout_stale = false;
                    shared.signals().plugin_dialog_state.set(ds);
                    fresh
                } else {
                    ctx.request_repaint();
                    stale
                }
            }
            (None, _) => match try_fetch_layout(
                shared,
                &plugin_id,
                arclain_plugins::types::PluginExtensionPoint::Dialog(dialog_id.clone()),
            ) {
                Some(fresh) => {
                    let mut ds = shared.signals().plugin_dialog_state.get();
                    ds.cached_dialog_layout = Some(fresh.clone());
                    shared.signals().plugin_dialog_state.set(ds);
                    fresh
                }
                None => {
                    ctx.request_repaint();
                    loading_placeholder_layout()
                }
            },
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

    // Send __page_init event if this page was just opened (for SetPageDisplayName etc).
    //
    // This path uses the BLOCKING dispatch variant because any
    // `SetPageDisplayName` action returned has to be applied to the
    // breadcrumb signal BEFORE this same frame paints the breadcrumb.
    // Pushing into shared.pending_plugin_actions and waiting for the
    // next render would render once with the wrong title.
    {
        let needs_init = shared.signals().plugin_dialog_state.get().page_needs_init;
        if needs_init {
            if let Some(pm_arc) = &shared.services.plugin_manager {
                use parking_lot::Mutex as PlMutex;
                use std::sync::Arc as StdArc;
                let local_sink: StdArc<PlMutex<Vec<(String, arclain_plugins::types::PluginAction)>>> =
                    StdArc::new(PlMutex::new(Vec::new()));
                let pm = pm_arc.lock();
                let ran = crate::features::plugins::presentation::dispatch::dispatch_plugin_event_blocking(
                    &pm,
                    &plugin_id,
                    "__page_init",
                    Some(page_id.clone()),
                    &local_sink,
                );
                drop(pm); // Release lock before processing actions

                // Only clear page_needs_init when the dispatch actually
                // ran. If it bailed because a worker was mid-event we
                // leave the flag set so the next frame retries — and
                // request_repaint to make sure the next frame happens
                // soon (otherwise repaint may not fire until input).
                if ran {
                    let actions: Vec<arclain_plugins::types::PluginAction> = local_sink
                        .lock()
                        .drain(..)
                        .map(|(_, a)| a)
                        .collect();
                    let mut ds = shared.signals().plugin_dialog_state.get();
                    ds.page_needs_init = false;
                    if !actions.is_empty() {
                        let mut toaster = shared.toaster.lock();
                        let render_tab = shared.signals().tabs.get().active().clone();
                        let ctx = crate::features::plugins::presentation::controllers::plugin_controller::ActionContext {
                            lightbox_signal: Some(&shared.signals().lightbox_state),
                            page_display_name_signal: Some(&render_tab.page_display_name),
                            shared_state: Some(shared),
                        };
                        for action in actions {
                            crate::features::plugins::presentation::controllers::plugin_controller::process_action(
                                action,
                                &plugin_id,
                                &mut ds,
                                &mut toaster,
                                None,
                                &ctx,
                            );
                        }
                    }
                    shared.signals().plugin_dialog_state.set(ds);
                } else {
                    ctx.request_repaint();
                }
            }
        }
    }

    // Resolve the page layout for this frame. Same three-case shape
    // as render_dialog above:
    //   - cache + not stale → use cache directly
    //   - cache + stale     → try refetch; keep stale on contention
    //                         so the user doesn't see the page blank
    //                         out while a worker is mid-event (e.g.
    //                         clicking Refetch while a fetch is
    //                         already running)
    //   - no cache          → try fetch; on busy show "Loading…"
    //                         placeholder (e.g. drop-archive
    //                         auto-fetch with the DLSite tab open)
    let is_stale = shared.signals().plugin_dialog_state.get().cached_page_layout_stale;
    let page_layout = match (cached_layout, is_stale) {
        (Some(layout), false) => layout,
        (Some(stale), true) => {
            if let Some(fresh) = try_fetch_layout(
                shared,
                &plugin_id,
                arclain_plugins::types::PluginExtensionPoint::Page(page_id.clone()),
            ) {
                let mut ds = shared.signals().plugin_dialog_state.get();
                ds.cached_page_layout = Some(fresh.clone());
                ds.cached_page_layout_stale = false;
                shared.signals().plugin_dialog_state.set(ds);
                fresh
            } else {
                ctx.request_repaint();
                stale
            }
        }
        (None, _) => match try_fetch_layout(
            shared,
            &plugin_id,
            arclain_plugins::types::PluginExtensionPoint::Page(page_id.clone()),
        ) {
            Some(fresh) => {
                let mut ds = shared.signals().plugin_dialog_state.get();
                ds.cached_page_layout = Some(fresh.clone());
                shared.signals().plugin_dialog_state.set(ds);
                fresh
            }
            None => {
                ctx.request_repaint();
                loading_placeholder_layout()
            }
        },
    };

    // Get display name from signal, fallback to page_id
    let display_name = shared
        .signals()
        .tabs
        .get()
        .active()
        .page_display_name
        .get()
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
            let mut callback = crate::features::plugins::presentation::controllers::plugin_controller::create_page_callback(shared, plugin_id.clone());


            use arclain_plugins::types::PluginLayout;
            let content_cache = shared.services.content_cache.clone();

            match page_layout {
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
