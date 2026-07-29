//! Applies what a facade-rendered plugin document produced: the
//! [`DocumentEvent`]s a render frame returned, and the
//! [`PluginHostIntentDto`]s an action result carried back.
//!
//! Every extension point that renders through
//! `crate::features::plugins::application::facade_sessions` routes through
//! here, so navigation and intents behave identically no matter which slot
//! produced them -- the property the pre-cutover stack could not have,
//! because each call site handled its own subset of reserved event-id
//! prefixes (see `facade_sessions`'s module doc comment).

use arclain_app::event::OperationEvent;
use arclain_app::plugins::{PluginActionDto, PluginHostIntentDto, PluginToastLevelDto};
use arclain_widgets::{Toast, ToastLevel};

use crate::core::tabs::TabId;
use crate::features::plugins::application::{PluginNavigation, PluginSlot};
use crate::features::plugins::presentation::rendering::DocumentEvent;
use crate::shared::dialogs::LightboxState;
use crate::shared::image_assets::ImageOwner;
use crate::shared::SharedState;

/// Applies every event one document render produced.
///
/// `origin_tab` is the tab that owns the slot -- navigation opens dialogs
/// and pages against it rather than against whichever tab happens to be
/// active when an asynchronous result lands.
pub fn apply_document_events(
    shared: &SharedState,
    slot: &PluginSlot,
    origin_tab: TabId,
    events: Vec<DocumentEvent>,
) {
    for event in events {
        match event {
            DocumentEvent::Navigate(navigation) => {
                apply_navigation(shared, slot.plugin_id(), origin_tab, navigation);
            }
            DocumentEvent::Interact { node_id, action } => {
                dispatch_action(shared, slot, node_id, action);
            }
        }
    }
}

/// Starts one plugin interaction and makes sure its result is routed back
/// to `slot` even if the facade finishes it before this call returns.
///
/// The registry cannot track an operation id it does not have yet, and
/// `start_plugin_action`'s worker runs concurrently with the caller
/// resuming from that `.await` -- so a fast action can reach `Completed`
/// and be broadcast before `track` records it, and the bridge would drop
/// that terminal event as belonging to no slot. Re-reading the
/// operation's own snapshot immediately after tracking closes the window:
/// whatever state it reached in between is observed here instead of being
/// lost. `crate::core::operation_bridge::register_operation` solves the
/// identical race for archive operations the identical way, and the
/// registry's own routing rules make the double delivery harmless -- the
/// second arrival finds the operation already drained.
pub fn dispatch_action(
    shared: &SharedState,
    slot: &PluginSlot,
    node_id: String,
    action: PluginActionDto,
) {
    let Some(facade) = shared.facade.clone() else {
        return;
    };
    let shared = shared.clone();
    let slot = slot.clone();
    shared.services.tokio_runtime.clone().spawn(async move {
        let Some(operation_id) = shared
            .plugin_sessions
            .start_action(&facade, &slot, node_id, action)
            .await
        else {
            return;
        };
        reconcile_started_action(&shared, &facade, operation_id).await;
    });
}

/// Routes an action operation's *current* state through the bridge, right
/// after it was tracked.
///
/// This is the race closer: if the operation already reached a terminal
/// state before [`dispatch_action`] recorded its id, the broadcast event
/// announcing it was dropped as belonging to no slot and nothing else will
/// ever deliver it. Re-reading the operation's own snapshot recovers
/// exactly that case. If it is still running this is a no-op, and the live
/// broadcast delivers the terminal event normally.
///
/// A named function rather than inline so the condition it exists for --
/// "already terminal by the time we look" -- can be exercised
/// deterministically, which asserting through [`dispatch_action`] cannot
/// be: that function starts the operation, so whether it has finished a
/// few microseconds later is genuinely a race (measured at 1 failure in 3
/// runs), and with no broadcast subscriber attached a test that loses the
/// race waits forever rather than merely being slow. The re-read is in
/// fact *usually* the loser -- it is one dispatch round trip, while the
/// action worker additionally takes a per-plugin lock and makes a WASM
/// call -- which is exactly why it is a recovery path and not the primary
/// one.
///
/// So this function's behavior is pinned by driving it with an
/// already-terminal operation, and its single call site above is not
/// independently pinned. Two ways to close that were tried and rejected:
/// a log-based witness (`tracing_test` scopes captured events to the
/// test's own thread, and this runs on a spawned task -- verified with a
/// throwaway probe), and a counter on the registry (test-only state on a
/// production type, to pin one line).
pub async fn reconcile_started_action(
    shared: &SharedState,
    facade: &arclain_app::ArclainApp,
    operation_id: arclain_app::ids::OperationId,
) {
    let Ok(snapshot) = facade.operation(operation_id).await else {
        return;
    };
    // Recorded unconditionally, before the outcome is known: whether this
    // re-read *recovers* anything depends on whether the operation
    // happened to finish first, so the useful diagnostic is that the
    // recovery attempt ran at all. Host-derived fields only.
    tracing::debug!(
        operation_id = operation_id.into_raw(),
        "[plugin-sessions] re-read a started action's own snapshot"
    );
    crate::core::operation_bridge::handle_plugin_action_event(
        shared,
        OperationEvent {
            operation_id: snapshot.operation_id,
            sequence: snapshot.last_sequence,
            kind: snapshot.kind,
            state: snapshot.state,
        },
    );
}

/// Host navigation, applied to this frontend's own dialog/page state.
///
/// Deliberately *not* part of the session registry: which dialog is open
/// and what the page back-stack holds is renderer-owned state that
/// outlives any individual plugin session, and keeping it here is what
/// lets a facade-rendered panel open a dialog that still renders through
/// the legacy path.
pub fn apply_navigation(
    shared: &SharedState,
    plugin_id: &str,
    origin_tab: TabId,
    navigation: PluginNavigation,
) {
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let mut state = dialog_signal.get();
    match navigation {
        PluginNavigation::OpenDialog { dialog_id } => {
            state.open_dialog(plugin_id, &dialog_id, origin_tab);
            dialog_signal.set(state);
        }
        PluginNavigation::CloseDialog => {
            let owner = state
                .open_dialog
                .as_ref()
                .map(|(plugin_id, dialog_id, tab)| {
                    ImageOwner::plugin_dialog(plugin_id, dialog_id, *tab)
                });
            state.close_dialog();
            dialog_signal.set(state);
            if let Some(owner) = owner {
                shared.image_assets.release_owner(&owner);
            }
        }
        PluginNavigation::OpenPage { page_id } => {
            state.open_page(plugin_id, &page_id, origin_tab);
            // A newly pushed page inherits no display name from the page
            // it covers -- the plugin sets its own via
            // `SetPageDisplayName`, and until it does the page id is the
            // heading.
            page_display_name(shared, origin_tab).set(None);
            dialog_signal.set(state);
        }
        PluginNavigation::ClosePage => {
            let owner = state
                .current_page()
                .map(|(plugin_id, page_id, tab)| ImageOwner::plugin_page(plugin_id, page_id, tab));
            state.close_page();
            dialog_signal.set(state);
            page_display_name(shared, origin_tab).set(None);
            if let Some(owner) = owner {
                shared.image_assets.release_owner(&owner);
            }
        }
    }
}

/// Applies the bounded host intents one `start_plugin_action` result
/// carried back.
///
/// `PluginHostIntentDto` has no `RefreshPanel` or `RequestFetch` variant:
/// the application layer resolves both itself (a refresh is folded into
/// the same dispatch that requested it, so the updated document already
/// reflects it), which is why nothing here re-fetches anything.
pub fn apply_intents(
    shared: &SharedState,
    slot: &PluginSlot,
    origin_tab: TabId,
    intents: Vec<PluginHostIntentDto>,
) {
    for intent in intents {
        apply_intent(shared, slot, origin_tab, intent);
    }
}

pub fn apply_intent(
    shared: &SharedState,
    slot: &PluginSlot,
    origin_tab: TabId,
    intent: PluginHostIntentDto,
) {
    match intent {
        PluginHostIntentDto::ShowToast { message, level } => {
            shared
                .toaster
                .lock()
                .add(Toast::new(toast_level(level), message));
        }
        PluginHostIntentDto::CopyToClipboard { text } => match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(error) = clipboard.set_text(&text) {
                    tracing::error!("Failed to copy to clipboard: {error}");
                    shared
                        .toaster
                        .lock()
                        .error(format!("Failed to copy: {error}"));
                }
            }
            Err(error) => {
                tracing::error!("Failed to access clipboard: {error}");
                shared
                    .toaster
                    .lock()
                    .error(format!("Clipboard unavailable: {error}"));
            }
        },
        PluginHostIntentDto::OpenLightbox {
            images,
            start_index,
            title,
        } => {
            let tabs = shared.signals().tabs.get();
            let Some(tab) = tabs.get(origin_tab).cloned() else {
                return;
            };
            drop(tabs);
            // The lightbox reads its images through the same
            // `ImageAssetStore` the document renderer does, so the
            // facade-encoded cache keys carry through unchanged.
            let images = images
                .into_iter()
                .map(|image| (image.cache_key, image.url))
                .collect();
            tab.lightbox_state.set(LightboxState::open(
                images,
                start_index as usize,
                title,
                Some(slot.plugin_id().to_string()),
            ));
        }
        PluginHostIntentDto::SetPageDisplayName { name } => {
            page_display_name(shared, origin_tab).set(Some(name));
        }
        PluginHostIntentDto::CloseDialog => {
            apply_navigation(
                shared,
                slot.plugin_id(),
                origin_tab,
                PluginNavigation::CloseDialog,
            );
        }
    }
}

fn page_display_name(
    shared: &SharedState,
    origin_tab: TabId,
) -> arclain_app::Signal<Option<String>> {
    let tabs = shared.signals().tabs.get();
    tabs.get(origin_tab)
        .unwrap_or_else(|| tabs.active())
        .page_display_name
        .clone()
}

fn toast_level(level: PluginToastLevelDto) -> ToastLevel {
    match level {
        PluginToastLevelDto::Info => ToastLevel::Info,
        PluginToastLevelDto::Success => ToastLevel::Success,
        PluginToastLevelDto::Warning => ToastLevel::Warning,
        PluginToastLevelDto::Error => ToastLevel::Error,
    }
}
