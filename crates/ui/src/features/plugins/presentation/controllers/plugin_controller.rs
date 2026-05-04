//! Plugin action handler
//!
//! Processes `PluginAction` values returned from plugin UI events
//! and dispatches them to the appropriate subsystems.

use arclain_plugins::types::{PluginAction, ToastLevel as PluginToastLevel};
use arclain_signals::Signal;
use arclain_widgets::{Toast, ToastLevel, Toaster};
use parking_lot::Mutex;
use std::sync::Arc;

use crate::features::plugins::domain::state::PluginDialogState;
use crate::shared::dialogs::LightboxState;

/// Context for processing plugin actions that need signal access
pub struct ActionContext<'a> {
    pub lightbox_signal: Option<&'a Signal<LightboxState>>,
    pub page_display_name_signal: Option<&'a Signal<Option<String>>>,
    pub shared_state: Option<&'a crate::shared::SharedState>,
}

/// Process a list of plugin actions.
///
/// Pass `shared_state` so that actions which need to spawn background work
/// (notably `RequestFetch` and `CacheContent`) can reach the tokio runtime,
/// the gameta client, and the plugin manager. Without it, those actions are
/// silently no-op'd — the historic toolbar default for years, which is why
/// "Fetch DLSite" looked like it ran but never actually fetched anything.
pub fn process_plugin_actions(
    actions: Vec<PluginAction>,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
    lightbox_signal: Option<&Signal<LightboxState>>,
    shared_state: Option<&crate::shared::SharedState>,
) {
    let ctx = ActionContext {
        lightbox_signal,
        page_display_name_signal: None,
        shared_state,
    };
    for action in actions {
        process_action(action, plugin_id, dialog_state, toaster, refresh_requests, &ctx);
    }
}

/// Process a single plugin action
pub fn process_action(
    action: PluginAction,
    plugin_id: &str,
    _dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
    ctx: &ActionContext,
) {
    match action {
        PluginAction::None => {}

        PluginAction::EmitMetadata { json: _ } => {
            // Metadata emission is handled separately in the plugin event flow
            tracing::debug!("Plugin {} emitted metadata via action", plugin_id);
        }

        PluginAction::CacheContent { key, url } => {
            // Cache requests should be handled by a background task
            tracing::debug!(
                "Plugin {} requested caching key='{}' from url='{}'",
                plugin_id,
                key,
                url
            );
            // TODO: Queue for background download
        }

        PluginAction::ShowToast { message, level } => {
            let toast_level = match level {
                PluginToastLevel::Info => ToastLevel::Info,
                PluginToastLevel::Success => ToastLevel::Success,
                PluginToastLevel::Warning => ToastLevel::Warning,
                PluginToastLevel::Error => ToastLevel::Error,
            };

            toaster.add(Toast::new(toast_level, message));
        }

        PluginAction::ShowMessage { title, message } => {
            // For now, show as a toast; could be upgraded to a modal dialog
            toaster.info(format!("{}: {}", title, message));
        }

        PluginAction::RefreshPanel { extension_point } => {
            // Queue panel for refresh - UI components check this list
            tracing::debug!(
                "Plugin {} requested refresh of panel '{}'",
                plugin_id,
                extension_point
            );
            if let Some(requests) = refresh_requests {
                requests.lock().push(extension_point);
            }
        }

        PluginAction::UpdateElement { id, value } => {
            // Element updates are typically handled by plugin-side state
            tracing::debug!(
                "Plugin {} requested update of element '{}' to '{}'",
                plugin_id,
                id,
                value
            );
            // TODO: Implement element update if needed for server-driven UI
        }

        PluginAction::OpenPage { page } => {
            // Navigate to the plugin page
            tracing::info!(
                "Plugin {} requested navigation to page '{}'",
                plugin_id,
                page
            );
            _dialog_state.open_page(plugin_id, &page);
        }

        PluginAction::CloseDialog => {
            // Close the current dialog
            tracing::debug!("Plugin {} requested dialog close", plugin_id);
            _dialog_state.close_dialog();
        }

        PluginAction::CopyToClipboard { text } => {
            // Copy text to system clipboard
            tracing::debug!("Plugin {} requested clipboard copy: {}", plugin_id, text);
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
            tracing::debug!(
                "Plugin {} set page display name to '{}'",
                plugin_id,
                name
            );
            if let Some(signal) = ctx.page_display_name_signal {
                signal.set(Some(name));
            }
        }

        PluginAction::RequestFetch { key } => {
            // Async background fetch — host handles on tokio runtime
            tracing::info!("Plugin {} requested background fetch: {}", plugin_id, key);
            if let Some(shared) = ctx.shared_state {
                spawn_background_fetch(shared, &plugin_id, &key);
            }
        }
    }
}

/// Create a callback handler for plugin dialog events
pub fn create_dialog_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
) -> Box<dyn FnMut(&str, Option<String>)> {
    // Use signal instead of Arc<Mutex>
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let lightbox_signal = shared.signals().lightbox_state.clone();
    let page_display_name_signal = shared.signals().page_display_name.clone();
    let toaster_arc = shared.toaster.clone();
    let plugin_manager_arc = shared.services.plugin_manager.clone();
    // Owned clone so we can hand a reference into ActionContext from within
    // the 'static closure below — without this, RequestFetch / CacheContent
    // and other shared-state-dependent actions silently no-op.
    let shared_owned = shared.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        // Check for close dialog signal
        if element_id == "__dialog_close" {
            let mut ds = dialog_signal.get();
            ds.close_dialog();
            dialog_signal.set(ds);
            return;
        }

        // Normal event - use plugin_manager from services
        if let Some(pm_arc) = &plugin_manager_arc {
            let pm = pm_arc.lock();
            if let Some(actions) = pm
                .with_plugin_instance(&pid, |instance| {
                    instance.send_ui_event(element_id, value).ok()
                })
                .flatten()
            {
                drop(pm); // Release plugin manager lock before locking toaster
                let mut toaster = toaster_arc.lock();

                // Get state from signal, modify, and write back
                let mut ds = dialog_signal.get();
                // Invalidate layout cache so next frame fetches fresh layout
                ds.invalidate_dialog_layout();
                let ctx = ActionContext {
                    lightbox_signal: Some(&lightbox_signal),
                    page_display_name_signal: Some(&page_display_name_signal),
                    shared_state: Some(&shared_owned),
                };
                for action in actions {
                    process_action(
                        action,
                        &pid,
                        &mut ds,
                        &mut toaster,
                        None, // No refresh requests for dialog callbacks
                        &ctx,
                    );
                }
                dialog_signal.set(ds);
            }
        }
    })
}

/// Create a callback handler for plugin page events
pub fn create_page_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
) -> Box<dyn FnMut(&str, Option<String>)> {
    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let lightbox_signal = shared.signals().lightbox_state.clone();
    let page_display_name_signal = shared.signals().page_display_name.clone();
    let toaster_arc = shared.toaster.clone();
    let plugin_manager_arc = shared.services.plugin_manager.clone();
    // Owned clone so the closure can pass a reference to ActionContext;
    // without this, RequestFetch (etc.) silently no-ops because process_action
    // skips its body when ctx.shared_state is None.
    let shared_owned = shared.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        // Check for close page signal
        if element_id == "__page_close" {
            let mut ds = dialog_signal.get();
            ds.close_page();
            // Clear page display name when closing
            page_display_name_signal.set(None);
            dialog_signal.set(ds);
            return;
        }

        // Check for open page signal (nested navigation)
        if element_id.starts_with("__page_open:") {
            let new_page_id = element_id.trim_start_matches("__page_open:").to_string();
            let mut ds = dialog_signal.get();
            ds.open_page(&pid, &new_page_id);
            // Clear display name for new page (plugin will set it)
            page_display_name_signal.set(None);
            dialog_signal.set(ds);
            return;
        }

        // Normal event - use plugin_manager from services
        if let Some(pm_arc) = &plugin_manager_arc {
            let pm = pm_arc.lock();
            if let Some(actions) = pm
                .with_plugin_instance(&pid, |instance| {
                    instance.send_ui_event(element_id, value).ok()
                })
                .flatten()
            {
                drop(pm);
                let mut toaster = toaster_arc.lock();
                let mut ds = dialog_signal.get();
                // Invalidate layout cache so next frame fetches fresh layout
                ds.invalidate_page_layout();
                let ctx = ActionContext {
                    lightbox_signal: Some(&lightbox_signal),
                    page_display_name_signal: Some(&page_display_name_signal),
                    shared_state: Some(&shared_owned),
                };
                for action in actions {
                    process_action(
                        action,
                        &pid,
                        &mut ds,
                        &mut toaster,
                        None, // No refresh requests for page callbacks
                        &ctx,
                    );
                }
                dialog_signal.set(ds);
            }
        }
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
) {
    // Parse "source:id" format
    let parts: Vec<&str> = key.splitn(2, ':').collect();
    let (source, id) = if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("dlsite".to_string(), key.to_string())
    };

    let plugin_id_owned = plugin_id.to_string();
    let metadata_signal = shared.signals().metadata.clone();

    // Send fetch-completion event back to the plugin. Called when the gameta
    // server succeeds; the plugin clears its in_progress flag in response.
    let make_complete_notifier = || {
        let pid = plugin_id_owned.clone();
        let pm = shared.services.plugin_manager.clone();
        let key = key.to_string();
        move |success: bool| {
            if let Some(pm) = pm {
                let event = if success {
                    format!("background_fetch_complete:{}", key)
                } else {
                    format!("background_fetch_failed:{}", key)
                };
                let manager = pm.lock();
                manager.with_plugin_instance(&pid, |instance| {
                    let _ = instance.send_ui_event(&event, None);
                });
            }
        }
    };

    // Hand the work off to the plugin's own HTTP path (uses the host's
    // SOCKS5-aware client). The plugin's `do_native_fetch:` handler clears
    // its in-progress flag on completion, so no separate notifier needed.
    let make_native_dispatcher = || {
        let pid = plugin_id_owned.clone();
        let pm = shared.services.plugin_manager.clone();
        let key = key.to_string();
        move || {
            if let Some(pm) = pm {
                let event = format!("do_native_fetch:{}", key);
                let manager = pm.lock();
                manager.with_plugin_instance(&pid, |instance| {
                    let _ = instance.send_ui_event(&event, None);
                });
            }
        }
    };

    // Try gameta server (fully async, no plugin mutex). On any kind of
    // failure (server missing, 404, parse error), fall through to the plugin's
    // native HTTP fetch so the user actually gets metadata when their
    // gameta_server isn't configured or doesn't have this entry.
    if let Some(ref client) = shared.services.gameta_client {
        let client = client.clone();
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
                match client.get_metadata(&source_clone, &id_clone) {
                    Ok(Some(meta)) => Ok(Some(meta)),
                    Ok(None) => client
                        .fetch_metadata(&source_clone, &id_clone, false)
                        .map(|r| r.metadata),
                    Err(e) => Err(e),
                }
            })
            .await;

            match result {
                Ok(Ok(Some(meta))) => match serde_json::to_value(&meta) {
                    Ok(json_val) => {
                        tracing::info!("[BackgroundFetch] Got {} from server", id_for_log);
                        metadata_signal.set(Some(json_val));
                        notify_complete(true);
                    }
                    Err(_) => {
                        tracing::warn!(
                            "[BackgroundFetch] {} fetched but JSON serialization failed — falling back to native fetch",
                            id_for_log
                        );
                        spawn_native_dispatch(dispatch_native, &id_for_log, notifier_json_fail)
                            .await;
                    }
                },
                Ok(Ok(None)) => {
                    tracing::info!(
                        "[BackgroundFetch] {} not on gameta server — falling back to native fetch",
                        id_for_log
                    );
                    spawn_native_dispatch(dispatch_native, &id_for_log, notifier_no_meta).await;
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "[BackgroundFetch] gameta server fetch failed for {}: {} — falling back to native fetch",
                        id_for_log,
                        e
                    );
                    spawn_native_dispatch(dispatch_native, &id_for_log, notifier_server_err)
                        .await;
                }
                Err(join_err) => {
                    tracing::error!(
                        "[BackgroundFetch] blocking task panicked for {}: {}",
                        id_for_log,
                        join_err
                    );
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
        tracing::warn!(
            "[BackgroundFetch] no plugin manager available, dropping fetch request"
        );
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
async fn spawn_native_dispatch<F, N>(dispatch: F, id_for_log: &str, notify_on_failure: N)
where
    F: FnOnce() + Send + 'static,
    N: FnOnce(bool) + Send + 'static,
{
    match tokio::task::spawn_blocking(dispatch).await {
        Ok(()) => {}
        Err(e) if e.is_panic() => {
            tracing::error!(
                "[BackgroundFetch] native dispatch panicked for {}: {}",
                id_for_log,
                e
            );
            notify_on_failure(false);
        }
        Err(e) => {
            tracing::error!(
                "[BackgroundFetch] native dispatch task error for {}: {}",
                id_for_log,
                e
            );
            notify_on_failure(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::Arc;

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
    #[tokio::test]
    async fn m5_panic_in_dispatch_calls_notify_with_false() {
        let notified_with = Arc::new(AtomicU8::new(0)); // 0 = not called
        let n = notified_with.clone();
        spawn_native_dispatch(
            || panic!("simulated plugin panic"),
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
