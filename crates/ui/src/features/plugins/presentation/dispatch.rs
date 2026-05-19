//! Centralized plugin-event dispatcher.
//!
//! Every UI path that fires a plugin event must go through
//! `dispatch_plugin_event`. The helper:
//! - locks the plugin instance on a tokio blocking thread (NOT the UI
//!   thread), so the WASM call (which may include synchronous polling
//!   for HTTP) never freezes the UI;
//! - pushes returned actions into `SharedState::pending_plugin_actions`;
//! - persists settings if the plugin's `settings_dirty` flag flipped
//!   during the event.
//!
//! Direct calls to `instance.send_ui_event(...)` from the UI thread are
//! a regression and should be replaced with this helper.

use crate::shared::SharedState;
use arclain_plugins::PluginManager;
use parking_lot::Mutex;
use std::sync::Arc;

/// Fire-and-forget plugin event dispatch.
///
/// Returns immediately. Actions land in `shared.pending_plugin_actions`
/// on a future frame; settings auto-save to disk if the plugin marked
/// them dirty.
///
/// v1 intentional omission: plugin calls do NOT increment `in_flight_ops`
/// on the originating tab. Plugin events are typically short-lived (ms to
/// low seconds) and the overhead of OpGuard wiring through every dispatch
/// call site outweighs the benefit at this stage. If a tab is force-closed
/// while a plugin call is in flight, the call runs to completion against the
/// captured `Arc<TabState>` but the user can't observe the result. A future
/// audit pass can add OpGuard here if plugin calls become long-running.
pub fn dispatch_plugin_event(
    shared: &SharedState,
    plugin_id: String,
    event_id: String,
    value: Option<String>,
) {
    let Some(pm_arc) = shared.services.plugin_manager.clone() else {
        return;
    };
    let app_state = shared.app_state.clone();
    let cfg_svc = shared.services.config_service.clone();
    let tokio_runtime = shared.services.tokio_runtime.clone();
    let sink = shared.pending_plugin_actions.clone();

    let work = move || {
        let (settings_to_save, actions) = {
            let mgr = pm_arc.lock();
            if let Some(instance_arc) = mgr.get_plugin_instance(&plugin_id) {
                let mut instance = instance_arc.lock();
                let actions = instance.send_ui_event(&event_id, value).ok();
                drop(instance);
                let snapshot = mgr.get_settings_for(&plugin_id);
                (snapshot, actions)
            } else {
                (None, None)
            }
        };

        if let Some(actions) = actions {
            let mut s = sink.lock();
            for a in actions {
                s.push((plugin_id.clone(), a));
            }
        }

        if let Some(settings_to_save) = settings_to_save {
            let mut state = app_state.lock();
            state
                .user_config
                .set_plugin_settings(&plugin_id, settings_to_save);

            if let Some(ref svc) = cfg_svc {
                if let Err(e) = svc.save_user_config(&state.user_config) {
                    tracing::error!("Failed to save plugin settings: {}", e);
                }
            }
        }
    };

    tokio_runtime.spawn(async move {
        let _ = tokio::task::spawn_blocking(work).await;
    });
}

/// Synchronous-when-possible variant for code paths that *must* see
/// the actions before the next paint (e.g. opening a plugin page
/// where the breadcrumb display name action must apply this frame).
///
/// Returns `true` if the event ran (lock was free). Returns `false`
/// if the instance lock was held by a worker (e.g. mid-fetch); the
/// caller should leave whatever flag triggered the dispatch SET so
/// the next frame retries — that way the UI doesn't freeze waiting
/// for a long-running plugin event to finish, but the action still
/// runs eventually.
pub fn dispatch_plugin_event_blocking(
    manager: &PluginManager,
    plugin_id: &str,
    event_id: &str,
    value: Option<String>,
    sink: &Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>>,
) -> bool {
    let Some(instance_arc) = manager.get_plugin_instance(plugin_id) else {
        return true; // No instance to dispatch to — treat as "done".
    };
    let Some(mut instance) = instance_arc.try_lock() else {
        // Worker thread is mid-event (e.g. holding the lock during a
        // DLSite fetch). Bail rather than freeze the UI; caller
        // retries on the next frame.
        return false;
    };
    if let Ok(actions) = instance.send_ui_event(event_id, value) {
        let mut s = sink.lock();
        for a in actions {
            s.push((plugin_id.to_string(), a));
        }
    }
    true
}
