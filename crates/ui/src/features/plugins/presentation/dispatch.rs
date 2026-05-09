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

/// Synchronous variant for code paths that *must* see the actions
/// immediately (e.g. opening a plugin page where the layout fetch
/// depends on `__page_init` having run). Locks the UI thread.
///
/// AVOID where possible — only used for `__page_init`-style events
/// that the UI synchronously needs the response from. The callers'
/// motivation is documented inline at each use site.
pub fn dispatch_plugin_event_blocking(
    manager: &PluginManager,
    plugin_id: &str,
    event_id: &str,
    value: Option<String>,
    sink: &Arc<Mutex<Vec<(String, arclain_plugins::types::PluginAction)>>>,
) {
    let Some(instance_arc) = manager.get_plugin_instance(plugin_id) else {
        return;
    };
    let mut instance = instance_arc.lock();
    if let Ok(actions) = instance.send_ui_event(event_id, value) {
        let mut s = sink.lock();
        for a in actions {
            s.push((plugin_id.to_string(), a));
        }
    }
}
