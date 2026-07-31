//! Plugin rendering module
//!
//! Contains functions for rendering plugin dialogs and pages,
//! extracted from arclain_app.rs to keep feature logic self-contained.
//!
//! Both [`render_dialog`] and [`render_page`] draw facade-session
//! documents (see
//! `crate::features::plugins::application::facade_sessions`) while
//! reading their navigation state from the same
//! `crate::features::plugins::domain::state::PluginDialogState`, which is
//! what lets a dialog button open a page and vice versa.

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

/// Returns the visible page after removing any page-stack entries whose
/// origin tab no longer exists.
///
/// The tab/session sweeper closes a dead tab's facade slot, but navigation
/// state is renderer-owned. Leaving the entry here would make the next
/// frame reopen the just-swept session forever, so the renderer that owns
/// the stack also owns pruning it.
fn dismiss_pages_whose_tabs_closed(shared: &SharedState) -> Option<(String, String, TabId)> {
    loop {
        let current = shared
            .signals()
            .plugin_dialog_state
            .get()
            .current_page()
            .map(|(plugin, page, tab)| (plugin.to_string(), page.to_string(), tab));
        let Some((plugin_id, page_id, origin_tab)) = current else {
            return None;
        };
        if shared.signals().tabs.get().get(origin_tab).is_some() {
            return Some((plugin_id, page_id, origin_tab));
        }

        let owner = ImageOwner::plugin_page(&plugin_id, &page_id, origin_tab);
        let signal = shared.signals().plugin_dialog_state.clone();
        let mut state = signal.get();
        state.close_page();
        signal.set(state);
        shared.image_assets.release_owner(&owner);
    }
}

fn start_page_initialization(
    shared: &SharedState,
    slot: &PluginSlot,
    page_id: &str,
    origin_tab: TabId,
) -> bool {
    let signal = shared.signals().plugin_dialog_state.clone();
    let mut state = signal.get();
    if !state.begin_page_initialization(slot.plugin_id(), page_id, origin_tab) {
        return false;
    }
    signal.set(state);

    let Some(facade) = shared.facade.clone() else {
        return true;
    };
    let shared = shared.clone();
    let slot = slot.clone();
    let page_id = page_id.to_string();
    shared
        .services
        .tokio_runtime
        .handle()
        .clone()
        .spawn(async move {
            let Some(operation_id) = shared
                .plugin_sessions
                .start_page_init(&facade, &slot, page_id)
                .await
            else {
                return;
            };
            document_dispatch::reconcile_started_action(&shared, &facade, operation_id).await;
        });
    true
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
    let page_info = dismiss_pages_whose_tabs_closed(shared);
    let slot = page_info
        .as_ref()
        .map(|(plugin_id, page_id, origin_tab)| PluginSlot::Page {
            plugin_id: plugin_id.clone(),
            page_id: page_id.clone(),
            tab: *origin_tab,
        });
    if let Some(facade) = shared.facade.as_ref() {
        shared.plugin_sessions.retain_open_page(
            facade,
            shared.services.tokio_runtime.handle(),
            slot.as_ref(),
        );
    }

    let Some(((plugin_id, page_id, origin_tab), slot)) = page_info.zip(slot) else {
        return false;
    };
    let image_owner = ImageOwner::plugin_page(plugin_id.clone(), page_id.clone(), origin_tab);
    let mut view = shared.facade.as_ref().map(|facade| {
        shared
            .plugin_sessions
            .view(facade, shared.services.tokio_runtime.handle(), &slot)
    });
    if matches!(view, Some(SlotView::Failed(_))) {
        let signal = shared.signals().plugin_dialog_state.clone();
        let mut state = signal.get();
        if state.mark_page_unavailable(&plugin_id, &page_id, origin_tab) {
            signal.set(state);
        }
    }
    if matches!(view, Some(SlotView::Ready(_)))
        && start_page_initialization(shared, &slot, &page_id, origin_tab)
    {
        // The document returned by opening the session is pre-init and
        // must never be drawn. Its action result publishes revision 2,
        // which the operation bridge stores before marking init complete.
        view = Some(SlotView::Opening);
    }

    // Get display name from signal, fallback to page_id
    let display_name = shared
        .signals()
        .tabs
        .get()
        .get(origin_tab)
        .and_then(|tab| tab.page_display_name.get())
        .unwrap_or_else(|| page_id.clone());

    // Render as full page content
    let mut events = Vec::new();
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
            let page_init_error = shared.signals().plugin_dialog_state.get().page_init_error();
            match (view.as_ref(), page_init_error) {
                (_, Some(error)) => {
                    ui.label(
                        egui::RichText::new(format!("Plugin page initialization failed: {error}"))
                            .color(shared.theme.colors.error),
                    );
                }
                (Some(SlotView::Ready(document)), None)
                    if shared
                        .signals()
                        .plugin_dialog_state
                        .get()
                        .page_layout_ready() =>
                {
                    let context = DocumentContext {
                        colors: &shared.theme.colors,
                        shared_state: Some(shared),
                        image_owner: Some(&image_owner),
                        extent: DocumentExtent::Full,
                    };
                    match &document.root.kind {
                        arclain_app::plugins::PluginUiNodeKind::Single { .. } => {
                            Form::new()
                                .id(format!("plugin_page_single_{page_id}"))
                                .margin(16.0)
                                .show(ui, &shared.theme, |ui| {
                                    events = render_document(ui, document, context);
                                });
                        }
                        arclain_app::plugins::PluginUiNodeKind::Split { .. } => {
                            egui::Frame::NONE.inner_margin(16.0).show(ui, |ui| {
                                events = render_document(ui, document, context);
                            });
                        }
                        _ => unreachable!("normalized plugin document roots are containers"),
                    }
                }
                (Some(SlotView::Failed(error)), None) => {
                    ui.label(
                        egui::RichText::new(format!("Plugin UI error: {error}"))
                            .color(shared.theme.colors.error),
                    );
                }
                (Some(SlotView::Opening | SlotView::Ready(_)), None) => {
                    ui.label(
                        egui::RichText::new("Loading plugin UI…")
                            .color(shared.theme.colors.on_surface_variant),
                    );
                }
                (None, None) => {
                    ui.label(
                        egui::RichText::new("Plugin UI is unavailable.")
                            .color(shared.theme.colors.on_surface_variant),
                    );
                }
            }
        });

    if !events.is_empty() {
        document_dispatch::apply_document_events(shared, &slot, origin_tab, events);
    }

    true
}
