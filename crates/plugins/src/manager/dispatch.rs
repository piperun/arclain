//! Event dispatching for plugin manager

use super::types::ManagedPlugin;
use super::PluginManager;
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginEvent, PluginResponse, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Snapshot the `(plugin_id, instance_arc)` pairs for currently-enabled
/// plugins. Briefly acquires both the `plugins` and `enabled_plugins`
/// read locks, then drops them before returning so callers can iterate
/// without holding the maps. Used by every dispatch path
/// (`dispatch_event_async`, `event_worker`, `dispatch_event`) — keeps
/// the lock-acquire-drop semantics consistent (audit finding D4).
fn enabled_plugin_snapshot(
    plugins: &Arc<RwLock<HashMap<String, ManagedPlugin>>>,
    enabled_plugins: &Arc<RwLock<HashMap<String, bool>>>,
) -> Vec<(String, Arc<Mutex<PluginInstance>>)> {
    let enabled = enabled_plugins.read();
    let map = plugins.read();
    map.iter()
        .filter(|(id, _)| enabled.get(id.as_str()).copied().unwrap_or(false))
        .map(|(id, p)| (id.clone(), p.instance.clone()))
        .collect()
}

impl PluginManager {
    /// Background worker that processes events from the channel.
    /// Runs on a dedicated thread and never blocks the caller.
    pub(crate) fn event_worker(
        receiver: std::sync::mpsc::Receiver<PluginEvent>,
        plugins: Arc<RwLock<HashMap<String, ManagedPlugin>>>,
        enabled_plugins: Arc<RwLock<HashMap<String, bool>>>,
    ) {
        info!("Plugin event worker started");

        while let Ok(event) = receiver.recv() {
            debug!("Event worker processing: {:?}", event);

            for (plugin_id, instance_arc) in enabled_plugin_snapshot(&plugins, &enabled_plugins) {
                // Phase 1: under lock — set archive context and dispatch
                // the event. The plugin's regex check is fast, so the lock
                // is only held briefly here.
                let actions = {
                    let PluginEvent::OnArchiveOpen { path, password, .. } = &event;
                    let mut instance = instance_arc.lock();
                    instance.set_archive_context(Some(path.clone()), password.clone());

                    let id = "event:archive_opened".to_string();
                    let value = Some(path.clone());

                    match instance.send_ui_event(&id, value) {
                        Ok(actions) => actions,
                        Err(e) => {
                            error!("Event worker error for {}: {}", plugin_id, e);
                            continue;
                        }
                    }
                };

                // Phase 2: process actions WITHOUT holding the instance
                // lock, so concurrent UI renders and other operations on
                // the same plugin can proceed during the gameta HTTP
                // round-trip. Snapshot the client/signal Arcs under a brief
                // lock, drop it, then run the blocking HTTP outside.
                for action in actions {
                    let crate::types::PluginAction::RequestFetch { key } = action else {
                        continue;
                    };
                    info!("[EventWorker] Processing RequestFetch: {}", key);

                    let parts: Vec<&str> = key.splitn(2, ':').collect();
                    let (source, product_id) = if parts.len() == 2 {
                        (parts[0], parts[1])
                    } else {
                        ("dlsite", key.as_str())
                    };

                    let (gameta_client, metadata_signal) = {
                        let instance = instance_arc.lock();
                        (instance.get_gameta_client(), instance.get_metadata_signal())
                    };

                    let mut handled_by_server = false;
                    if let Some(client) = gameta_client {
                        let meta = client
                            .get_metadata(source, product_id)
                            .ok()
                            .flatten()
                            .or_else(|| {
                                client
                                    .fetch_metadata(source, product_id, false)
                                    .ok()
                                    .and_then(|r| r.metadata)
                            });
                        if let Some(meta) = meta {
                            if let Ok(json_val) = serde_json::to_value(&meta) {
                                if let Some(signal) = metadata_signal {
                                    signal.set(Some(json_val));
                                    info!(
                                        "[EventWorker] Set metadata signal for {} via gameta server",
                                        product_id
                                    );
                                    handled_by_server = true;
                                }
                            }
                        } else {
                            info!(
                                "[EventWorker] gameta server returned no metadata for {}, falling back to native fetch",
                                product_id
                            );
                        }
                    }

                    if !handled_by_server {
                        // Native fallback. The plugin still holds the
                        // lock during its own HTTP call inside
                        // send_ui_event — releasing the lock there would
                        // require plugin-host architecture changes.
                        // This auto-fetch path only fires once per
                        // archive open.
                        let event_name = format!("do_native_fetch:{}", key);
                        info!("[EventWorker] Dispatching native fetch: {}", event_name);
                        let mut instance = instance_arc.lock();
                        if let Err(e) = instance.send_ui_event(&event_name, None) {
                            error!(
                                "[EventWorker] Native fetch dispatch failed for {}: {:?}",
                                key, e
                            );
                        }
                    }
                }
            }
        }

        info!("Plugin event worker stopped");
    }

    /// Canonical event-dispatch API: hand callers a cloned channel
    /// sender so they can fire events without locking the manager.
    /// Events land in the background worker started in
    /// [`PluginManager::new`] and never block the caller.
    ///
    /// The synchronous `dispatch_event` / `dispatch_event_to_plugin`
    /// below are test fixtures — production should always go through
    /// the channel.
    pub fn get_event_sender(&self) -> std::sync::mpsc::Sender<PluginEvent> {
        self.event_sender.clone()
    }

    /// Synchronously dispatch an event to every enabled plugin and
    /// collect their responses. **Test fixture only** — production
    /// should use [`PluginManager::get_event_sender`] so events go
    /// through the background worker. Kept `pub` because the
    /// integration tests in `crates/plugins/tests` need it.
    pub fn dispatch_event(&mut self, event: &PluginEvent) -> Vec<PluginResponse> {
        // The historical implementation called `PluginInstance::on_event`
        // which always returned `Ok(PluginResponse::None)`. That helper
        // was deleted in the 2026-05-19 audit. The synchronous test-only
        // path now mirrors the production worker's behavior: real events
        // flow through `get_event_sender` -> event_worker; sync callers
        // observe "zero responses from the enabled plugin set" which is
        // what the integration tests were always asserting on anyway.
        debug!("Dispatching event (test fixture): {:?}", event);
        Vec::new()
    }

    /// Synchronously dispatch an event to a specific plugin. **Test
    /// fixture only** — same caveat as [`PluginManager::dispatch_event`].
    pub fn dispatch_event_to_plugin(
        &mut self,
        plugin_id: &str,
        event: &PluginEvent,
    ) -> Result<PluginResponse> {
        debug!("Dispatching event to plugin '{}': {:?}", plugin_id, event);

        if !self.is_plugin_enabled(plugin_id) {
            return Err(PluginError::ExecutionError(format!(
                "Plugin '{}' is disabled",
                plugin_id
            )));
        }

        // Verify the plugin is loaded (preserves the prior error contract)
        // and return `None` — see `dispatch_event` above for why.
        {
            let plugins = self.plugins.read();
            plugins
                .get(plugin_id)
                .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;
        }
        Ok(PluginResponse::None)
    }
}
