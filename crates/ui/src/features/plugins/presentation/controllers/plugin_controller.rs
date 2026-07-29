//! Plugin action handler
//!
//! Processes `PluginAction` values returned from plugin UI events
//! and dispatches them to the appropriate subsystems.

use arclain_app::Signal;
use arclain_plugins::action_policy::bound_plugin_actions_with_status;
use arclain_plugins::types::{PluginAction, ToastLevel as PluginToastLevel};
use arclain_widgets::{Toast, ToastLevel, Toaster};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::features::plugins::domain::state::PluginDialogState;
use crate::shared::dialogs::LightboxState;
use crate::shared::image_assets::ImageOwner;

const PLUGIN_ACTIONS_LIMITED_WARNING: &str =
    "Plugin actions were limited because host safety limits were exceeded";

/// Context for processing plugin actions that need signal access
pub struct ActionContext<'a> {
    pub lightbox_signal: Option<&'a Signal<LightboxState>>,
    pub page_display_name_signal: Option<&'a Signal<Option<String>>>,
    pub metadata_signal: Option<&'a Signal<Option<serde_json::Value>>>,
    pub shared_state: Option<&'a crate::shared::SharedState>,
    pub origin_tab: Option<crate::core::tabs::TabId>,
}

fn trace_request_fetch_requested(plugin_id: &str, key: &str) {
    let _ = key;
    tracing::info!(plugin_id, "Plugin requested background metadata fetch");
}

fn trace_refresh_panel_requested(plugin_id: &str, extension_point: &str) {
    let _ = extension_point;
    tracing::debug!(plugin_id, "Plugin requested panel refresh");
}

fn trace_clipboard_copy_requested(plugin_id: &str, text: &str) {
    let _ = text;
    tracing::debug!(plugin_id, "Plugin requested clipboard copy");
}

fn trace_page_display_name_changed(plugin_id: &str, name: &str) {
    let _ = name;
    tracing::debug!(plugin_id, "Plugin changed page display name");
}

/// Execute the UI RequestFetch route only when the loaded instance has both
/// capabilities required to perform a network request and write its result.
/// The closure is the entire effectful route, so denial cannot start Gameta
/// work or mutate an archive metadata signal.
fn process_ui_request_fetch<Authorize, Fetch>(
    authorize: Authorize,
    plugin_id: &str,
    start_fetch: Fetch,
) -> bool
where
    Authorize: FnOnce() -> Result<(), String>,
    Fetch: FnOnce(),
{
    if let Err(error) = authorize() {
        tracing::warn!(
            plugin_id,
            %error,
            "Denied RequestFetch without Network and ArchiveMetadataWrite capabilities"
        );
        return false;
    }

    start_fetch();
    true
}

fn run_gameta_request_sequence<T>(
    mut acquire_permit: impl FnMut() -> Result<(), String>,
    get_metadata: impl FnOnce() -> Result<Option<T>, String>,
    fetch_metadata: impl FnOnce() -> Result<Option<T>, String>,
) -> Result<Option<T>, String> {
    acquire_permit()?;
    if let Some(metadata) = get_metadata()? {
        return Ok(Some(metadata));
    }
    acquire_permit()?;
    fetch_metadata()
}

/// Process a list of plugin actions.
///
/// Pass `shared_state` so that actions which need to spawn background work
/// (notably `RequestFetch`) can reach the tokio runtime, the gameta client,
/// and the plugin manager. Without it, those actions are silently no-op'd —
/// the historic toolbar default for years, which is why "Fetch DLSite"
/// looked like it ran but never actually fetched anything.
pub fn process_plugin_actions(
    actions: Vec<PluginAction>,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<AtomicBool>>,
    lightbox_signal: Option<&Signal<LightboxState>>,
    shared_state: Option<&crate::shared::SharedState>,
) {
    let ctx = ActionContext {
        lightbox_signal,
        page_display_name_signal: None,
        metadata_signal: None,
        shared_state,
        origin_tab: shared_state.map(|shared| shared.signals().tabs.get().active_id()),
    };
    process_plugin_actions_with_context(
        actions,
        plugin_id,
        dialog_state,
        toaster,
        refresh_requests,
        &ctx,
    );
}

pub fn process_plugin_actions_with_context(
    actions: Vec<PluginAction>,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<AtomicBool>>,
    ctx: &ActionContext,
) {
    process_plugin_actions_with_limit_status(
        actions,
        false,
        plugin_id,
        dialog_state,
        toaster,
        refresh_requests,
        ctx,
    );
}

pub(crate) fn process_plugin_actions_with_limit_status(
    actions: Vec<PluginAction>,
    actions_were_limited: bool,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<AtomicBool>>,
    ctx: &ActionContext,
) {
    let bounded = bound_plugin_actions_with_status(actions);
    if actions_were_limited || bounded.limited {
        toaster.warning(PLUGIN_ACTIONS_LIMITED_WARNING);
    }
    for action in bounded.actions {
        process_bounded_action(
            action,
            plugin_id,
            dialog_state,
            toaster,
            refresh_requests,
            ctx,
        );
    }
}

/// Process a single plugin action
pub fn process_action(
    action: PluginAction,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<AtomicBool>>,
    ctx: &ActionContext,
) {
    let bounded = bound_plugin_actions_with_status(vec![action]);
    if bounded.limited {
        toaster.warning(PLUGIN_ACTIONS_LIMITED_WARNING);
    }
    for action in bounded.actions {
        process_bounded_action(
            action,
            plugin_id,
            dialog_state,
            toaster,
            refresh_requests,
            ctx,
        );
    }
}

fn process_bounded_action(
    action: PluginAction,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<AtomicBool>>,
    ctx: &ActionContext,
) {
    match action {
        PluginAction::None => {}

        PluginAction::ShowToast { message, level } => {
            let toast_level = match level {
                PluginToastLevel::Info => ToastLevel::Info,
                PluginToastLevel::Success => ToastLevel::Success,
                PluginToastLevel::Warning => ToastLevel::Warning,
                PluginToastLevel::Error => ToastLevel::Error,
            };

            toaster.add(Toast::new(toast_level, message));
        }

        PluginAction::RefreshPanel { extension_point } => {
            trace_refresh_panel_requested(plugin_id, &extension_point);
            if let Some(requests) = refresh_requests {
                requests.store(true, Ordering::Release);
            }
        }

        PluginAction::CloseDialog => {
            // Close the current dialog
            tracing::debug!("Plugin {} requested dialog close", plugin_id);
            let image_owner =
                dialog_state
                    .open_dialog
                    .as_ref()
                    .map(|(plugin_id, dialog_id, tab_id)| {
                        ImageOwner::plugin_dialog(plugin_id, dialog_id, *tab_id)
                    });
            dialog_state.close_dialog();
            if let (Some(shared), Some(owner)) = (ctx.shared_state, image_owner) {
                shared.image_assets.release_owner(&owner);
            }
        }

        PluginAction::CopyToClipboard { text } => {
            // Copy text to system clipboard
            trace_clipboard_copy_requested(plugin_id, &text);
            match arboard::Clipboard::new() {
                Ok(mut clipboard) => {
                    if let Err(e) = clipboard.set_text(&text) {
                        tracing::error!("Failed to copy to clipboard: {}", e);
                        toaster.error(format!("Failed to copy: {}", e));
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to access clipboard: {}", e);
                    toaster.error(format!("Clipboard unavailable: {}", e));
                }
            }
        }

        PluginAction::OpenLightbox {
            images,
            start_index,
            title,
        } => {
            // Open the lightbox with the provided images
            tracing::debug!(
                "Plugin {} requested lightbox with {} images starting at index {}",
                plugin_id,
                images.len(),
                start_index
            );
            if let Some(signal) = ctx.lightbox_signal {
                let state = LightboxState::open(images, start_index, title);
                signal.set(state);
            } else {
                tracing::warn!("Lightbox requested but signal not available");
            }
        }

        PluginAction::SetPageDisplayName { name } => {
            // Set the display name for the current plugin page
            trace_page_display_name_changed(plugin_id, &name);
            if let Some(signal) = ctx.page_display_name_signal {
                signal.set(Some(name));
            }
        }

        PluginAction::RequestFetch { key } => {
            // Async background fetch — host handles on tokio runtime
            trace_request_fetch_requested(plugin_id, &key);
            if let Some(shared) = ctx.shared_state {
                let origin_tab = ctx
                    .origin_tab
                    .unwrap_or_else(|| shared.signals().tabs.get().active_id());
                process_ui_request_fetch(
                    || {
                        let manager = shared
                            .services
                            .plugin_manager
                            .as_ref()
                            .ok_or_else(|| "plugin manager unavailable".to_string())?;
                        if !manager.lock().plugin_has_capabilities(
                            plugin_id,
                            &arclain_plugins::types::REQUEST_FETCH_CAPABILITIES,
                        ) {
                            return Err(
                                "Network and ArchiveMetadataWrite capabilities are required"
                                    .to_string(),
                            );
                        }
                        Ok(())
                    },
                    plugin_id,
                    || {
                        spawn_background_fetch(
                            shared,
                            plugin_id,
                            &key,
                            origin_tab,
                            ctx.metadata_signal.cloned(),
                        );
                    },
                );
            }
        }
    }
}

/// Create a callback handler for plugin dialog events.
///
/// Plugin events are queued with their origin tab through `PluginUiJobs`.
/// The central completion path applies returned actions and settings to
/// that same tab even if the user switches before WASM returns.
/// Layout invalidation is a pure signal mutation and stays inline.
pub fn create_dialog_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
    origin_tab: crate::core::tabs::TabId,
) -> Box<dyn FnMut(&str, Option<String>)> {
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let shared_owned = shared.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        if element_id == "__dialog_close" {
            let mut ds = dialog_signal.get();
            let image_owner = ds
                .open_dialog
                .as_ref()
                .map(|(plugin_id, dialog_id, tab_id)| {
                    ImageOwner::plugin_dialog(plugin_id, dialog_id, *tab_id)
                });
            ds.close_dialog();
            dialog_signal.set(ds);
            if let Some(owner) = image_owner {
                shared_owned.image_assets.release_owner(&owner);
            }
            return;
        }

        crate::features::plugins::presentation::dispatch::dispatch_plugin_event_for_tab(
            &shared_owned,
            origin_tab,
            pid.clone(),
            element_id.to_string(),
            value,
        );

        // Drop cached layout so the next render fetches a fresh one
        // (the plugin may have changed its layout in response).
        let mut ds = dialog_signal.get();
        ds.invalidate_dialog_layout();
        dialog_signal.set(ds);
    })
}

/// Create a callback handler for plugin page events. See
/// `create_dialog_callback` for the dispatch model — this is the
/// page-level analogue with one extra prefix (`__page_open:`) handled
/// inline because it's a UI navigation, not a plugin event.
pub fn create_page_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
    origin_tab: crate::core::tabs::TabId,
) -> Box<dyn FnMut(&str, Option<String>)> {
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let tabs = shared.signals().tabs.get();
    let page_display_name_signal = tabs
        .get(origin_tab)
        .unwrap_or_else(|| tabs.active())
        .page_display_name
        .clone();
    drop(tabs);
    let shared_owned = shared.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        if element_id == "__page_close" {
            let mut ds = dialog_signal.get();
            let image_owner = ds.current_page().map(|(plugin_id, page_id, tab_id)| {
                ImageOwner::plugin_page(plugin_id, page_id, tab_id)
            });
            ds.close_page();
            page_display_name_signal.set(None);
            dialog_signal.set(ds);
            if let Some(owner) = image_owner {
                shared_owned.image_assets.release_owner(&owner);
            }
            return;
        }

        if let Some(new_page_id) = element_id.strip_prefix("__page_open:") {
            let mut ds = dialog_signal.get();
            ds.open_page(&pid, new_page_id, origin_tab);
            page_display_name_signal.set(None);
            dialog_signal.set(ds);
            return;
        }

        crate::features::plugins::presentation::dispatch::dispatch_plugin_event_for_tab(
            &shared_owned,
            origin_tab,
            pid.clone(),
            element_id.to_string(),
            value,
        );

        let mut ds = dialog_signal.get();
        ds.invalidate_page_layout();
        dialog_signal.set(ds);
    })
}

/// Spawn a background metadata fetch on the tokio runtime.
/// Uses gameta server if available, otherwise falls back to plugin event.
///
/// Always notifies the plugin via a `background_fetch_complete:{source}:{id}`
/// UI event when the fetch finishes (success OR failure) so the plugin can
/// clear its in-progress flag and re-check its cache. Silent hangs were
/// previously possible when the gameta-server path returned an error and the
/// plugin had no way to learn the fetch was over.
fn spawn_background_fetch(
    shared: &crate::shared::SharedState,
    plugin_id: &str,
    key: &str,
    origin_tab: crate::core::tabs::TabId,
    origin_metadata: Option<Signal<Option<serde_json::Value>>>,
) {
    let materialization_limit = shared
        .services
        .resource_manager
        .as_ref()
        .map_or(
            arclain_plugins::types::MAX_PLUGIN_METADATA_BYTES,
            |manager| manager.materialization_limit(),
        )
        .min(arclain_plugins::types::MAX_PLUGIN_METADATA_BYTES);
    // Parse "source:id" format
    let parts: Vec<&str> = key.splitn(2, ':').collect();
    let (source, id) = if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("dlsite".to_string(), key.to_string())
    };

    let plugin_id_owned = plugin_id.to_string();
    let metadata_signal = origin_metadata.unwrap_or_else(|| {
        let tabs = shared.signals().tabs.get();
        tabs.get(origin_tab)
            .unwrap_or_else(|| tabs.active())
            .metadata
            .clone()
    });

    // Send fetch-completion event back to the plugin. Called when the gameta
    // server succeeds; the plugin clears its in_progress flag in response.
    //
    // Completion notifications use the same ordered, origin-aware event
    // path as direct UI interactions so errors and returned actions are
    // never dropped.
    let make_complete_notifier = || {
        let pid = plugin_id_owned.clone();
        let jobs = shared.plugin_ui_jobs.clone();
        let key = key.to_string();
        move |success: bool| {
            let event = if success {
                format!("background_fetch_complete:{}", key)
            } else {
                format!("background_fetch_failed:{}", key)
            };
            jobs.request(
                crate::features::plugins::application::PluginUiRequest::ReactiveUiEvent {
                    plugin_id: pid,
                    event_id: event,
                    value: None,
                    origin_tab,
                },
            );
        }
    };

    // Hand the work off to the plugin's own HTTP path (uses the host's
    // SOCKS5-aware client). The plugin's `do_native_fetch:` handler clears
    // its in-progress flag on completion, so no separate notifier needed.
    //
    // Native fetch dispatch is likewise ordered with other plugin side
    // effects and retains the original tab context.
    let make_native_dispatcher = || {
        let pid = plugin_id_owned.clone();
        let jobs = shared.plugin_ui_jobs.clone();
        let key = key.to_string();
        move || {
            jobs.request(
                crate::features::plugins::application::PluginUiRequest::ReactiveUiEvent {
                    plugin_id: pid,
                    event_id: format!("do_native_fetch:{}", key),
                    value: None,
                    origin_tab,
                },
            );
        }
    };

    // Try gameta server (fully async, no plugin mutex). On any kind of
    // failure (server missing, 404, parse error), fall through to the plugin's
    // native HTTP fetch so the user actually gets metadata when their
    // gameta_server isn't configured or doesn't have this entry.
    if let Some(ref client) = shared.services.gameta_client {
        let client = client.clone();
        let policy_client = shared.services.async_http_client.clone();
        let permit_plugin_id = plugin_id_owned.clone();
        let id_for_log = id.clone();
        let source_clone = source.clone();
        let id_clone = id.clone();
        let notify_complete = make_complete_notifier();
        // Pre-create notifiers for each fallback arm. Each is FnOnce and
        // only one will fire per execution; the rest drop unused.
        // Pre-creating outside the async block avoids the lifetime issue
        // of calling the factory closure (which borrows local vars) from
        // inside `async move`.
        let notifier_json_fail = make_complete_notifier();
        let notifier_no_meta = make_complete_notifier();
        let notifier_server_err = make_complete_notifier();
        let dispatch_native = make_native_dispatcher();

        shared.services.tokio_runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                run_gameta_request_sequence(
                    || {
                        policy_client
                            .try_acquire_plugin_host_service(&permit_plugin_id, "gameta")
                            .map_err(|error| error.to_string())
                    },
                    || {
                        client
                            .get_metadata_with_limit(
                                &source_clone,
                                &id_clone,
                                materialization_limit,
                            )
                            .map_err(|error| error.to_string())
                    },
                    || {
                        client
                            .fetch_metadata_with_limit(
                                &source_clone,
                                &id_clone,
                                false,
                                materialization_limit,
                            )
                            .map(|response| response.metadata)
                            .map_err(|error| error.to_string())
                    },
                )
            })
            .await;

            match result {
                Ok(Ok(Some(meta))) => match serde_json::to_value(&meta) {
                    Ok(json_val)
                        if arclain_plugins::types::metadata_value_within_limit(&json_val) => {
                        tracing::info!("[BackgroundFetch] Received metadata from server");
                        metadata_signal.set(Some(json_val));
                        notify_complete(true);
                    }
                    Ok(_) | Err(_) => {
                        tracing::warn!(
                            "[BackgroundFetch] Metadata serialization failed; falling back to native fetch"
                        );
                        spawn_native_dispatch(dispatch_native, &id_for_log, notifier_json_fail)
                            .await;
                    }
                },
                Ok(Ok(None)) => {
                    tracing::info!(
                        "[BackgroundFetch] Metadata not on Gameta server; falling back to native fetch"
                    );
                    spawn_native_dispatch(dispatch_native, &id_for_log, notifier_no_meta).await;
                }
                Ok(Err(_)) => {
                    tracing::warn!(
                        "[BackgroundFetch] Gameta server fetch failed; falling back to native fetch"
                    );
                    spawn_native_dispatch(dispatch_native, &id_for_log, notifier_server_err)
                        .await;
                }
                Err(_) => {
                    tracing::error!("[BackgroundFetch] blocking task panicked");
                    notify_complete(false);
                }
            }
        });
        return;
    }

    // No gameta server — go straight to the plugin's native HTTP fetch.
    if shared.services.plugin_manager.is_some() {
        let dispatch_native = make_native_dispatcher();
        let notifier = make_complete_notifier();
        let id_for_log = id.clone();
        shared.services.tokio_runtime.spawn(async move {
            spawn_native_dispatch(dispatch_native, &id_for_log, notifier).await;
        });
    } else {
        tracing::warn!("[BackgroundFetch] no plugin manager available, dropping fetch request");
        let notify_plugin = make_complete_notifier();
        notify_plugin(false);
    }
}

/// Run a native-dispatch closure on a blocking thread and clear the
/// plugin's in-progress flag if the closure panics.
///
/// Audit finding M5: the previous shape was
/// `tokio::task::spawn_blocking(dispatch).await.ok()`, which silently
/// dropped both panics and other JoinErrors. The plugin's own handler
/// clears its in-progress flag at the end of its native fetch, so a
/// pre-flag-clear panic would leave the UI showing "fetching..."
/// forever.
///
/// On clean completion we call no notifier — the plugin handler has
/// already cleared its flag. On panic or other JoinError we log and
/// call `notify_on_failure(false)` so the flag clears via the
/// `background_fetch_failed:` event path.
async fn spawn_native_dispatch<F, N>(dispatch: F, _id_for_log: &str, notify_on_failure: N)
where
    F: FnOnce() + Send + 'static,
    N: FnOnce(bool) + Send + 'static,
{
    match tokio::task::spawn_blocking(dispatch).await {
        Ok(()) => {}
        Err(e) if e.is_panic() => {
            tracing::error!("[BackgroundFetch] native dispatch panicked");
            notify_on_failure(false);
        }
        Err(_) => {
            tracing::error!("[BackgroundFetch] native dispatch task error");
            notify_on_failure(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_plugins::action_policy::{
        bound_plugin_actions, MAX_LIGHTBOX_IMAGES, MAX_LIGHTBOX_TITLE_BYTES,
        MAX_REQUEST_FETCH_ACTIONS, MAX_TOAST_ACTIONS, MAX_TOAST_MESSAGE_BYTES,
    };
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::Arc;

    #[test]
    fn plugin_action_batch_is_bounded_coalesced_and_caps_guest_fields() {
        let mut actions = Vec::new();
        for index in 0..20 {
            actions.push(PluginAction::ShowToast {
                message: format!("toast-{index}-{}", "x".repeat(MAX_TOAST_MESSAGE_BYTES - 20)),
                level: PluginToastLevel::Info,
            });
            actions.push(PluginAction::RequestFetch {
                key: format!("fetch-{index}"),
            });
            actions.push(PluginAction::RefreshPanel {
                extension_point: format!("panel-{index}"),
            });
            actions.push(PluginAction::CopyToClipboard {
                text: format!("clipboard-{index}"),
            });
            actions.push(PluginAction::SetPageDisplayName {
                name: format!("page-{index}"),
            });
        }
        actions.push(PluginAction::OpenLightbox {
            images: (0..MAX_LIGHTBOX_IMAGES + 5)
                .map(|index| (format!("image-{index}"), None))
                .collect(),
            start_index: usize::MAX,
            title: Some("t".repeat(MAX_LIGHTBOX_TITLE_BYTES + 20)),
        });

        let bounded = bound_plugin_actions(actions);

        assert_eq!(
            bounded
                .iter()
                .filter(|action| matches!(action, PluginAction::ShowToast { .. }))
                .count(),
            MAX_TOAST_ACTIONS
        );
        assert_eq!(
            bounded
                .iter()
                .filter(|action| matches!(action, PluginAction::RequestFetch { .. }))
                .count(),
            MAX_REQUEST_FETCH_ACTIONS
        );
        assert_eq!(
            bounded
                .iter()
                .filter(|action| matches!(action, PluginAction::RefreshPanel { .. }))
                .count(),
            1
        );
        assert!(bounded.iter().any(|action| matches!(
            action,
            PluginAction::CopyToClipboard { text } if text == "clipboard-19"
        )));
        assert!(bounded.iter().any(|action| matches!(
            action,
            PluginAction::SetPageDisplayName { name } if name == "page-19"
        )));
        let lightbox = bounded.iter().find_map(|action| match action {
            PluginAction::OpenLightbox {
                images,
                start_index,
                title,
            } => Some((images, start_index, title)),
            _ => None,
        });
        let (images, start_index, title) = lightbox.expect("lightbox retained");
        assert_eq!(images.len(), MAX_LIGHTBOX_IMAGES);
        assert_eq!(*start_index, MAX_LIGHTBOX_IMAGES - 1);
        assert!(title.is_none());
        assert!(bounded.iter().all(|action| match action {
            PluginAction::ShowToast { message, .. } => message.len() <= MAX_TOAST_MESSAGE_BYTES,
            _ => true,
        }));
    }

    #[test]
    fn repeated_refresh_actions_retain_only_a_dirty_bit() {
        let refresh_pending = AtomicBool::new(false);
        let bounded = bound_plugin_actions(
            (0..100)
                .map(|index| PluginAction::RefreshPanel {
                    extension_point: format!("panel-{index}"),
                })
                .collect(),
        );

        for action in bounded {
            if matches!(action, PluginAction::RefreshPanel { .. }) {
                refresh_pending.store(true, Ordering::Release);
            }
        }

        assert!(refresh_pending.swap(false, Ordering::AcqRel));
        assert!(!refresh_pending.swap(false, Ordering::AcqRel));
    }

    #[test]
    fn denied_action_overflow_surfaces_one_host_warning() {
        let mut dialog = PluginDialogState::default();
        let mut toaster = Toaster::new();
        let context = ActionContext {
            lightbox_signal: None,
            page_display_name_signal: None,
            metadata_signal: None,
            shared_state: None,
            origin_tab: None,
        };

        process_plugin_actions_with_context(
            (0..20)
                .map(|_| PluginAction::ShowToast {
                    message: "x".repeat(MAX_TOAST_MESSAGE_BYTES + 1),
                    level: PluginToastLevel::Error,
                })
                .collect(),
            "ui-demo",
            &mut dialog,
            &mut toaster,
            None,
            &context,
        );

        assert_eq!(toaster.len(), 1);
        assert!(PLUGIN_ACTIONS_LIMITED_WARNING.contains("limited"));
    }

    #[test]
    fn field_capping_surfaces_one_limited_warning() {
        let mut dialog = PluginDialogState::default();
        let mut toaster = Toaster::new();
        let context = ActionContext {
            lightbox_signal: None,
            page_display_name_signal: None,
            metadata_signal: None,
            shared_state: None,
            origin_tab: None,
        };

        process_plugin_actions_with_context(
            vec![PluginAction::OpenLightbox {
                images: (0..MAX_LIGHTBOX_IMAGES + 1)
                    .map(|index| (format!("image-{index}"), None))
                    .collect(),
                start_index: usize::MAX,
                title: None,
            }],
            "ui-demo",
            &mut dialog,
            &mut toaster,
            None,
            &context,
        );

        assert_eq!(toaster.len(), 1);
    }

    #[test]
    fn prebounded_batch_preserves_its_limited_warning() {
        let mut dialog = PluginDialogState::default();
        let mut toaster = Toaster::new();
        let context = ActionContext {
            lightbox_signal: None,
            page_display_name_signal: None,
            metadata_signal: None,
            shared_state: None,
            origin_tab: None,
        };

        process_plugin_actions_with_limit_status(
            Vec::new(),
            true,
            "ui-demo",
            &mut dialog,
            &mut toaster,
            None,
            &context,
        );

        assert_eq!(toaster.len(), 1);
    }

    #[test]
    fn no_op_action_does_not_surface_a_limit_warning() {
        let mut dialog = PluginDialogState::default();
        let mut toaster = Toaster::new();
        let context = ActionContext {
            lightbox_signal: None,
            page_display_name_signal: None,
            metadata_signal: None,
            shared_state: None,
            origin_tab: None,
        };

        process_plugin_actions_with_context(
            vec![PluginAction::None],
            "ui-demo",
            &mut dialog,
            &mut toaster,
            None,
            &context,
        );

        assert!(toaster.is_empty());
    }

    #[test]
    fn refresh_actions_set_one_bounded_dirty_bit() {
        let dirty = Arc::new(AtomicBool::new(false));
        let mut dialog = PluginDialogState::default();
        let mut toaster = Toaster::new();
        let context = ActionContext {
            lightbox_signal: None,
            page_display_name_signal: None,
            metadata_signal: None,
            shared_state: None,
            origin_tab: None,
        };

        process_action(
            PluginAction::RefreshPanel {
                extension_point: "guest-value".repeat(100_000),
            },
            "ui-demo",
            &mut dialog,
            &mut toaster,
            Some(&dirty),
            &context,
        );

        assert!(dirty.load(Ordering::Acquire));
    }

    #[tracing_test::traced_test]
    #[test]
    fn request_fetch_trace_redacts_guest_key() {
        let marker = "plugin-request-key-must-not-reach-global-tracing";

        trace_request_fetch_requested("ui-demo", marker);

        assert!(!logs_contain(marker));
    }

    #[tracing_test::traced_test]
    #[test]
    fn plugin_action_traces_redact_guest_values() {
        let refresh_marker = "refresh-panel-value-must-not-reach-global-tracing";
        let clipboard_marker = "clipboard-text-must-not-reach-global-tracing";
        let display_name_marker = "page-display-name-must-not-reach-global-tracing";

        trace_refresh_panel_requested("ui-demo", refresh_marker);
        trace_clipboard_copy_requested("ui-demo", clipboard_marker);
        trace_page_display_name_changed("ui-demo", display_name_marker);

        assert!(!logs_contain(refresh_marker));
        assert!(!logs_contain(clipboard_marker));
        assert!(!logs_contain(display_name_marker));
    }

    #[test]
    fn ui_route_denies_request_fetch_without_both_capabilities() {
        for (network, metadata_write) in [(true, false), (false, true)] {
            let fetch_started = AtomicBool::new(false);
            let metadata = Signal::new(None);

            let authorized = process_ui_request_fetch(
                || {
                    if network && metadata_write {
                        Ok(())
                    } else {
                        Err("capability denied".to_string())
                    }
                },
                "ui-demo",
                || {
                    fetch_started.store(true, Ordering::SeqCst);
                    metadata.set(Some(serde_json::json!({"product_id": "RJ000001"})));
                },
            );

            assert!(!authorized);
            assert!(!fetch_started.load(Ordering::SeqCst));
            assert!(metadata.get().is_none());
        }
    }

    #[test]
    fn ui_route_allows_request_fetch_with_both_capabilities() {
        let fetch_started = AtomicBool::new(false);
        let metadata = Signal::new(None);

        let authorized = process_ui_request_fetch(
            || Ok(()),
            "ui-demo",
            || {
                fetch_started.store(true, Ordering::SeqCst);
                metadata.set(Some(serde_json::json!({"product_id": "RJ000001"})));
            },
        );

        assert!(authorized);
        assert!(fetch_started.load(Ordering::SeqCst));
        assert_eq!(
            metadata
                .get()
                .and_then(|value| value["product_id"].as_str().map(str::to_owned))
                .as_deref(),
            Some("RJ000001")
        );
    }

    #[test]
    fn ui_route_denies_request_fetch_when_host_service_budget_is_exhausted() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = arclain_network::AsyncHttpClient::new(
            runtime.handle().clone(),
            Arc::new(parking_lot::RwLock::new(
                arclain_network::DomainWhitelist::default(),
            )),
            None,
        );
        client.configure_plugin(
            "ui-demo",
            arclain_network::PluginNetworkPolicy {
                network_enabled: true,
                requests_per_minute: 1,
            },
        );
        let get_started = AtomicBool::new(false);
        let fetch_started = AtomicBool::new(false);

        let result = run_gameta_request_sequence(
            || {
                client
                    .try_acquire_plugin_host_service("ui-demo", "gameta")
                    .map_err(|error| error.to_string())
            },
            || {
                get_started.store(true, Ordering::SeqCst);
                Ok::<_, String>(None::<()>)
            },
            || {
                fetch_started.store(true, Ordering::SeqCst);
                Ok::<_, String>(None::<()>)
            },
        );

        assert!(result.is_err());
        assert!(get_started.load(Ordering::SeqCst));
        assert!(!fetch_started.load(Ordering::SeqCst));
    }

    /// Regression test for M5 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// Pre-fix, native-dispatch fallbacks ran as
    /// `tokio::task::spawn_blocking(dispatch).await.ok()`. The `.ok()`
    /// converted any `JoinError` (including panics from the dispatch
    /// closure) into `None` and the function moved on without telling
    /// anyone. The plugin's in-progress flag — which the dispatch
    /// closure was supposed to clear at the end of its work — would
    /// stay set forever, so the user's "Fetch DLSite" button looked
    /// stuck.
    ///
    /// Post-fix, `spawn_native_dispatch` matches on the JoinError and
    /// calls the supplied notifier with `false` so the plugin clears
    /// its flag via the `background_fetch_failed:` path.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn m5_panic_in_dispatch_calls_notify_with_false() {
        let panic_marker = "plugin-panic-payload-must-not-reach-global-tracing";
        let notified_with = Arc::new(AtomicU8::new(0)); // 0 = not called
        let n = notified_with.clone();
        spawn_native_dispatch(
            move || panic!("{panic_marker}"),
            "test_id",
            move |success| {
                n.store(if success { 1 } else { 2 }, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(
            notified_with.load(Ordering::SeqCst),
            2,
            "M5 fix regressed: notifier should have been called with false on panic",
        );
        assert!(!logs_contain(panic_marker));
    }

    /// Clean completion: notifier is NOT called (plugin handler clears
    /// its own flag during normal flow).
    #[tokio::test]
    async fn m5_clean_dispatch_does_not_invoke_notifier() {
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        spawn_native_dispatch(
            || { /* clean run */ },
            "test_id",
            move |_| {
                c.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert!(
            !called.load(Ordering::SeqCst),
            "Notifier must not be called when dispatch completes cleanly",
        );
    }
}
