//! The archive toolbar's plugin items, drawn from the plugin's
//! `PluginButton` session document.
//!
//! `crate::shared::components::toolbar` is deliberately plugin-agnostic:
//! it reads its own stored `UiItemDto`, parses that item's `action_data`
//! into a plugin id and an optional button id, and hands the pair to an
//! injected callback. `crate::core::arclain_app::toolbar_handler` builds
//! that callback out of [`render_toolbar_item`], so the toolbar module
//! never names a plugin type and this module never has to know how the
//! toolbar arranges itself.
//!
//! See `crate::features::plugins::application::facade_sessions` for the
//! session model this reads through, and for why declarative button
//! navigation resolves here rather than travelling to the plugin as a
//! reserved event-id string.

use arclain_app::plugins::{PluginActionDto, PluginUiDocument, PluginUiNodeKind};
use arclain_theme::ButtonVariant;
use eframe::egui;

use crate::features::plugins::application::{PluginNavigation, PluginSlot, SlotView};
use crate::features::plugins::presentation::document_dispatch;
use crate::features::plugins::presentation::rendering::{
    render_document, DocumentContext, DocumentEvent, DocumentExtent,
};
use crate::shared::image_assets::ImageOwner;
use crate::shared::SharedState;

/// Draws one plugin toolbar item and applies whatever the user did to it.
///
/// `button_id` is the specific button the stored item names, or `None`
/// for an item that names the plugin as a whole -- in which case the
/// plugin's whole toolbar document is drawn, the way the pre-cutover
/// "legacy multi-button" branch drew its whole flat element list.
///
/// One slot per plugin, window-scoped: the toolbar draws exactly one
/// instance of a plugin's `PluginButton` contribution for the whole
/// window (see [`PluginSlot`]'s own doc comment), so several items naming
/// the same plugin read one shared document rather than opening a session
/// each.
///
/// **No facade call happens per frame.** `PluginSessions::view` is a
/// registry lookup once the slot is open; the document changes only as
/// the result of a dispatched action, or a refresh that closed the
/// session. The path this replaced read a cache that a background worker
/// refilled with one `get-ui-layout` request per enabled plugin.
pub fn render_toolbar_item(
    ui: &mut egui::Ui,
    shared: &SharedState,
    plugin_id: &str,
    button_id: Option<&str>,
) {
    let Some(facade) = shared.facade.as_ref() else {
        return;
    };
    if !plugin_is_enabled(shared, plugin_id) {
        return;
    }
    let slot = PluginSlot::PluginButton {
        plugin_id: plugin_id.to_string(),
    };
    let SlotView::Ready(document) =
        shared
            .plugin_sessions
            .view(facade, shared.services.tokio_runtime.handle(), &slot)
    else {
        // Opening, or failed: draw nothing rather than a placeholder. A
        // toolbar is a strip of small controls with no room for one, and
        // the pre-cutover path also drew nothing for a plugin whose
        // layout had not arrived yet.
        return;
    };

    let events = match button_id {
        Some(button_id) => render_named_button(ui, shared, &document, button_id),
        None => render_document(
            ui,
            &document,
            DocumentContext {
                colors: &shared.theme.colors,
                shared_state: Some(shared),
                // Preserve the pre-cutover owner:
                // `ImageOwner::plugin_settings` is window-scoped, like this slot, so a
                // toolbar document's images are retained and evicted on
                // exactly the terms they always were.
                image_owner: Some(&ImageOwner::plugin_settings(plugin_id)),
                // A toolbar item owns its stretch of the toolbar rather
                // than being one section of a scrolling stack.
                extent: DocumentExtent::Full,
            },
        ),
    };
    if events.is_empty() {
        return;
    }
    // A `PluginButton` slot is window-scoped and has no tab of its own.
    // The toolbar is drawn for the active tab, so that is the tab a
    // dialog or page opened from it belongs to.
    let origin_tab = shared.signals().tabs.get().active_id();
    document_dispatch::apply_document_events(shared, &slot, origin_tab, events);
}

/// Whether `plugin_id` is currently enabled, per the frontend's cached
/// plugin snapshot.
///
/// Checked *before* the session is opened, and checked here at all,
/// because the application does not check it: `open_plugin_session`
/// resolves a plugin instance without consulting its enabled flag, so a
/// disabled plugin still answers `get-ui-layout`. A stored toolbar item
/// for a plugin the user has since disabled must draw nothing -- which is
/// what the pre-cutover path did by only fetching layouts for enabled
/// plugins, and what the enable toggle's own cache invalidation
/// (`views::detail_view`) is written to make happen.
///
/// A snapshot that has not loaded yet answers "no", the same conservative
/// answer the pre-cutover path gave; it arrives within a frame or two and
/// the toggle invalidates it, so this never goes stale in the direction
/// that would keep drawing a disabled plugin's button.
fn plugin_is_enabled(shared: &SharedState, plugin_id: &str) -> bool {
    let Some(Ok(plugins)) = shared.plugin_ui_jobs.plugin_snapshot() else {
        return false;
    };
    plugins
        .iter()
        .any(|plugin| plugin.id == plugin_id && plugin.enabled)
}

/// Draws the single `Button` node `button_id` names, in the toolbar's own
/// ghost styling rather than the document renderer's default -- a plugin
/// button sits among the host's own toolbar buttons and has to look like
/// one, which is why this is not `render_document` over a subtree.
///
/// Resolving through `PluginUiNodeDto::find` is what keeps a stored item
/// honest: an item naming a button the plugin no longer offers draws
/// nothing, exactly as it did when the lookup was a search through a flat
/// element list. It is also the lookup the application layer itself uses
/// to validate a dispatch, and the traversal
/// `crate::features::plugins::application::document_buttons` mirrors -- so
/// what the layout editor offers, what this can draw, and what the
/// application will accept are one set.
fn render_named_button(
    ui: &mut egui::Ui,
    shared: &SharedState,
    document: &PluginUiDocument,
    button_id: &str,
) -> Vec<DocumentEvent> {
    let Some(node) = document.root.find(button_id) else {
        return Vec::new();
    };
    let PluginUiNodeKind::Button { label, action } = &node.kind else {
        return Vec::new();
    };
    if !node.visible {
        return Vec::new();
    }

    // Scoped by session and button id so two plugins offering identically
    // labelled buttons cannot collide on an egui id -- the guarantee
    // `render_document` gets by pushing every node's id.
    let clicked = ui
        .push_id((document.session_id.into_raw(), button_id), |ui| {
            ui.add_enabled(
                node.enabled,
                // Plugin buttons keep their text: unlike the host's own
                // toolbar buttons they are unfamiliar and have no icon.
                arclain_widgets::TextButton::new(label, arclain_widgets::ButtonSize::Small)
                    .with_theme_colors(&shared.theme.colors)
                    .variant(ButtonVariant::Ghost),
            )
            .clicked()
        })
        .inner;
    if !clicked {
        return Vec::new();
    }

    // Declarative navigation resolves from the node's own typed action
    // and never reaches the plugin, at this call site as at every other --
    // see `facade_sessions`'s doc comment for the reserved-event-id
    // encoding this replaces.
    match PluginNavigation::resolve(button_id, action.as_ref()) {
        (Some(navigation), _) => vec![DocumentEvent::Navigate(navigation)],
        (None, Some(node_id)) => vec![DocumentEvent::Interact {
            expected_session_id: document.session_id,
            expected_revision: document.revision,
            node_id,
            action: PluginActionDto::Activate,
        }],
        (None, None) => Vec::new(),
    }
}
