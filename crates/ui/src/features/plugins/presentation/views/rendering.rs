//! Plugin rendering module
//!
//! Contains functions for rendering plugin dialogs and pages,
//! extracted from arclain_app.rs to keep feature logic self-contained.
//!
//! The two halves are on different stacks right now: [`render_dialog`]
//! draws a facade session's document (see
//! `crate::features::plugins::application::facade_sessions`), while
//! [`render_page`] still reads the legacy `PluginUiJobs` layout cache.
//! Both read their navigation state from the same
//! `crate::features::plugins::domain::state::PluginDialogState`, which is
//! what lets a dialog button open a page and vice versa while only one
//! side has moved.

use crate::core::tabs::TabId;
use crate::features::plugins::application::{PluginSlot, SlotView};
use crate::features::plugins::presentation::document_dispatch;
use crate::features::plugins::presentation::rendering::{
    render_document, DocumentContext, DocumentExtent,
};
use crate::shared::components::Form;
use crate::shared::image_assets::ImageOwner;
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

/// The open dialog, after dismissing one whose origin tab has closed.
///
/// A `Dialog` slot is tab-scoped, so
/// `crate::core::app_lifecycle::sweep_orphaned_plugin_sessions` closes it
/// as soon as its tab goes -- while the navigation entry naming that tab
/// survives, because nothing on the five tab-close paths touches it. The
/// two reconciles would then fight once per frame: the sweep closes the
/// session at the top of the frame and [`render_dialog`] re-opens it at
/// the bottom, which is one `get-ui-layout` call per frame for a dialog
/// belonging to an archive the user has left -- the exact defect the
/// session model removes everywhere else.
///
/// Dismissing is the honest resolution rather than merely the convenient
/// one: the dialog was opened by, and dispatches its events to, a tab
/// that no longer exists. The pre-cutover renderer left it on screen
/// serving a cached layout and sending its events to that dead tab.
fn dismiss_dialog_whose_tab_closed(shared: &SharedState) -> Option<(String, String, TabId)> {
    let open_dialog = shared.signals().plugin_dialog_state.get().open_dialog?;
    let (plugin_id, dialog_id, origin_tab) = &open_dialog;
    if shared.signals().tabs.get().get(*origin_tab).is_some() {
        return Some(open_dialog);
    }

    let image_owner = ImageOwner::plugin_dialog(plugin_id, dialog_id, *origin_tab);
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    state.close_dialog();
    signal.set(state);
    shared.image_assets.release_owner(&image_owner);
    None
}

/// Render the open plugin dialog, if any, as a modal overlay, and
/// reconcile the plugin session a dialog owns.
///
/// Called every frame from
/// `crate::core::arclain_app::dialog_handler::render_overlays` whether a
/// dialog is open or not, which is what lets the reconcile at the top
/// live here: this is the one place that runs on every frame and knows
/// which `Dialog` slot -- at most one -- still has a host. See
/// [`crate::features::plugins::application::PluginSessions::
/// retain_open_dialog`] for why the session lifetime is reconciled rather
/// than hooked into each of the four places a dialog can close.
pub fn render_dialog(ctx: &egui::Context, shared: &SharedState) {
    let dialog_info = dismiss_dialog_whose_tab_closed(shared);
    let slot = dialog_info
        .as_ref()
        .map(|(plugin_id, dialog_id, origin_tab)| PluginSlot::Dialog {
            plugin_id: plugin_id.clone(),
            dialog_id: dialog_id.clone(),
            tab: *origin_tab,
        });
    if let Some(facade) = shared.facade.as_ref() {
        shared.plugin_sessions.retain_open_dialog(
            facade,
            shared.services.tokio_runtime.handle(),
            slot.as_ref(),
        );
    }

    let Some(((plugin_id, dialog_id, origin_tab), slot)) = dialog_info.zip(slot) else {
        return;
    };
    let image_owner = ImageOwner::plugin_dialog(plugin_id, dialog_id.clone(), origin_tab);
    // `None` only without a facade, which a running application never is.
    // Reachable in tests, and it draws a line rather than an empty window
    // so the window itself (and its close button) still behaves.
    let view = shared.facade.as_ref().map(|facade| {
        shared
            .plugin_sessions
            .view(facade, shared.services.tokio_runtime.handle(), &slot)
    });

    let mut window_open = true;
    let mut events = Vec::new();
    egui::Window::new(format!("Plugin Dialog - {}", dialog_id))
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([400.0, 300.0])
        .open(&mut window_open)
        .show(ctx, |ui| match view.as_ref() {
            Some(SlotView::Ready(document)) => {
                events = render_document(
                    ui,
                    document,
                    DocumentContext {
                        colors: &shared.theme.colors,
                        shared_state: Some(shared),
                        image_owner: Some(&image_owner),
                        // A dialog owns the whole `Ui` inside its window,
                        // so a `Split` document may fill it -- the case
                        // `DocumentExtent::Full` describes exactly.
                        extent: DocumentExtent::Full,
                    },
                );
            }
            Some(SlotView::Opening) => {
                ui.label(
                    egui::RichText::new("Loading plugin UI…")
                        .color(shared.theme.colors.on_surface_variant),
                );
            }
            Some(SlotView::Failed(error)) => {
                ui.label(
                    egui::RichText::new(format!("Plugin UI error: {error}"))
                        .color(shared.theme.colors.error),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("Plugin UI is unavailable.")
                        .color(shared.theme.colors.on_surface_variant),
                );
            }
        });

    if !events.is_empty() {
        document_dispatch::apply_document_events(shared, &slot, origin_tab, events);
    }

    // Closed with the window's own X. Navigation state and images are
    // this function's to clear; the session is closed by the reconcile at
    // the top of the next frame, which is the single owner of that.
    if !window_open {
        let mut dialog_state = shared.signals().plugin_dialog_state.get();
        dialog_state.close_dialog();
        shared.signals().plugin_dialog_state.set(dialog_state);
        shared.image_assets.release_owner(&image_owner);
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
    let image_owner = ImageOwner::plugin_page(plugin_id.clone(), page_id.clone(), origin_tab);

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
    let page_init_error = shared.signals().plugin_dialog_state.get().page_init_error();
    let page_layout = if let Some(error) = page_init_error {
        message_layout(format!("Plugin page initialization failed: {error}"))
    } else if page_layout_ready {
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
            match page_layout.as_ref() {
                PluginLayout::Single { elements } => {
                    // Wrap in Form to provide ScrollArea (fixes cutoff bug)
                    Form::new()
                        .id(format!("plugin_page_single_{}", page_id))
                        .margin(16.0)
                        .show(ui, &shared.theme, |ui| {
                            super::ui::render_ui_elements_owned(
                                ui,
                                &elements,
                                &mut callback,
                                &shared.theme.colors,
                                Some(shared),
                                Some(&plugin_id),
                                Some(&image_owner),
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
                                            super::ui::render_ui_elements_owned(
                                                ui,
                                                &sidebar,
                                                &mut callback,
                                                &shared.theme.colors,
                                                Some(shared),
                                                Some(&plugin_id),
                                                Some(&image_owner),
                                            );
                                        });
                                });

                            egui::CentralPanel::default().show_inside(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt(format!("plugin_split_content_scroll_{}", page_id))
                                    .show(ui, |ui| {
                                        super::ui::render_ui_elements_owned(
                                            ui,
                                            &content,
                                            &mut callback,
                                            &shared.theme.colors,
                                            Some(shared),
                                            Some(&plugin_id),
                                            Some(&image_owner),
                                        );
                                    });
                            });
                        });
                }
            }
        });

    true
}
