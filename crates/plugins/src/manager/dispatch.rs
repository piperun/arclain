//! Event dispatching for plugin manager

use super::types::ManagedPlugin;
use super::PluginManager;
use crate::types::{PluginError, PluginEvent, PluginResponse, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info};

impl PluginManager {
    /// Send a UI event to a plugin asynchronously (non-blocking).
    /// The callback will be called on the background thread with the plugin's response.
    /// This prevents the UI from freezing during plugin execution.
    pub fn send_event_async<F>(
        &self,
        plugin_id: &str,
        event_id: &str,
        value: Option<String>,
        callback: F,
    ) where
        F: FnOnce(std::result::Result<Vec<crate::types::PluginUiElement>, String>) + Send + 'static,
    {
        // Get the plugin instance Arc before spawning thread
        let Some(instance_arc) = self.get_plugin_instance(plugin_id) else {
            callback(Err(format!("Plugin '{}' not found", plugin_id)));
            return;
        };

        let event_id = event_id.to_string();

        std::thread::spawn(move || {
            // Lock the instance on the background thread
            let mut instance = instance_arc.lock();

            match instance.send_ui_event(&event_id, value) {
                Ok(actions) => {
                    // Convert PluginAction to PluginUiElement for the callback
                    // For now, just pass an empty vec since actions are handled differently
                    callback(Ok(vec![]));

                    // Actions would need to be processed here or passed to a channel
                    // For UI refresh purposes, we'll handle this differently
                    if !actions.is_empty() {
                        tracing::debug!("Plugin returned {} actions (async)", actions.len());
                    }
                }
                Err(e) => {
                    callback(Err(format!("Plugin error: {:?}", e)));
                }
            }
        });
    }

    /// Dispatch an event to all enabled plugins asynchronously
    pub fn dispatch_event_async(&self, event: PluginEvent) {
        let plugins = self.plugins.clone();
        let enabled_plugins = self.enabled_plugins.clone();

        std::thread::spawn(move || {
            debug!("Async dispatching event: {:?}", event);

            // Collect enabled plugin instances in a single pass (2 lock acquisitions
            // instead of 2N).
            let dispatch_list: Vec<(String, _)> = {
                let enabled = enabled_plugins.read();
                let map = plugins.read();
                map.iter()
                    .filter(|(id, _)| enabled.get(*id).copied().unwrap_or(false))
                    .map(|(id, p)| (id.clone(), p.instance.clone()))
                    .collect()
            };

            for (plugin_id, instance_arc) in dispatch_list {
                let mut instance = instance_arc.lock();

                // Map PluginEvent to UI event for compatibility
                // (Since on_event is not exposed in WIT yet)
                if let PluginEvent::OnArchiveOpen { path, .. } = &event {
                    let id = "event:archive_opened".to_string();
                    let value = Some(path.clone());

                    if let Err(e) = instance.send_ui_event(&id, value) {
                        error!("Async event error for {}: {}", plugin_id, e);
                    }
                }
            }
        });
    }

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

            // Collect enabled plugin instances in a single pass (2 lock acquisitions
            // instead of 2N).
            let dispatch_list: Vec<(String, _)> = {
                let enabled = enabled_plugins.read();
                let map = plugins.read();
                map.iter()
                    .filter(|(id, _)| enabled.get(*id).copied().unwrap_or(false))
                    .map(|(id, p)| (id.clone(), p.instance.clone()))
                    .collect()
            };

            for (plugin_id, instance_arc) in dispatch_list {
                let mut instance = instance_arc.lock();

                // Match event to set context and dispatch
                match &event {
                    PluginEvent::OnArchiveOpen { path, password, .. } => {
                        // Set archive context (fast, no WASM call)
                        instance.set_archive_context(Some(path.clone()), password.clone());

                        // Dispatch to plugin (fast — plugin only does regex detection,
                        // returns RequestFetch action if code found)
                        let id = "event:archive_opened".to_string();
                        let value = Some(path.clone());

                        match instance.send_ui_event(&id, value) {
                            Ok(actions) => {
                                // Process RequestFetch actions on this background thread
                                for action in actions {
                                    if let crate::types::PluginAction::RequestFetch { key } = action {
                                        info!("[EventWorker] Processing RequestFetch: {}", key);

                                        let parts: Vec<&str> = key.splitn(2, ':').collect();
                                        let (source, product_id) = if parts.len() == 2 {
                                            (parts[0], parts[1])
                                        } else {
                                            ("dlsite", key.as_str())
                                        };

                                        // Try gameta server first, fall back to having the
                                        // plugin do its own fetch via the host's HTTP capability
                                        // when the server is missing or returns nothing.
                                        let mut handled_by_server = false;
                                        if let Some(ref client) = instance.get_gameta_client() {
                                            let meta = client.get_metadata(source, product_id)
                                                .ok()
                                                .flatten()
                                                .or_else(|| {
                                                    client.fetch_metadata(source, product_id, false)
                                                        .ok()
                                                        .and_then(|r| r.metadata)
                                                });
                                            if let Some(meta) = meta {
                                                if let Ok(json_val) = serde_json::to_value(&meta) {
                                                    if let Some(ref signal) = instance.get_metadata_signal() {
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
                                            // Ask the plugin to fetch via its own HTTP path. The
                                            // plugin holds the lock during the HTTP call, but
                                            // this is the auto-fetch path so it only fires once
                                            // per archive open.
                                            let event = format!("do_native_fetch:{}", key);
                                            info!("[EventWorker] Dispatching native fetch: {}", event);
                                            if let Err(e) = instance.send_ui_event(&event, None) {
                                                error!(
                                                    "[EventWorker] Native fetch dispatch failed for {}: {:?}",
                                                    key, e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Event worker error for {}: {:?}", plugin_id, e);
                            }
                        }
                    }
                    _ => {
                        // Other events (future)
                    }
                }
            }
        }

        info!("Plugin event worker stopped");
    }

    /// Send an event to all enabled plugins asynchronously.
    /// This method returns immediately - never blocks the caller.
    /// Events are processed by a background worker thread.
    pub fn send_event(&self, event: PluginEvent) {
        if let Err(e) = self.event_sender.send(event) {
            error!("Failed to send event to worker: {}", e);
        }
    }

    /// Get a cloned sender for lock-free event dispatch.
    /// Use this to avoid needing to lock the PluginManager when sending events.
    pub fn get_event_sender(&self) -> std::sync::mpsc::Sender<PluginEvent> {
        self.event_sender.clone()
    }

    /// Dispatch an event to all enabled plugins
    pub fn dispatch_event(&mut self, event: &PluginEvent) -> Vec<PluginResponse> {
        debug!("Dispatching event: {:?}", event);

        // Collect enabled plugin instances in a single pass (2 lock acquisitions
        // instead of 2N).
        let dispatch_list: Vec<(String, _)> = {
            let enabled = self.enabled_plugins.read();
            let map = self.plugins.read();
            map.iter()
                .filter(|(id, _)| enabled.get(*id).copied().unwrap_or(false))
                .map(|(id, p)| (id.clone(), p.instance.clone()))
                .collect()
        };

        let mut responses = Vec::new();
        for (plugin_id, instance_arc) in dispatch_list {
            let mut instance = instance_arc.lock();
            match instance.on_event(event) {
                Ok(response) => {
                    debug!("Plugin '{}' responded: {:?}", plugin_id, response);
                    responses.push(response);
                }
                Err(e) => {
                    error!("Plugin '{}' error handling event: {}", plugin_id, e);
                    responses.push(PluginResponse::Error {
                        message: e.to_string(),
                    });
                }
            }
        }

        responses
    }

    /// Dispatch event to a specific plugin
    pub fn dispatch_event_to_plugin(
        &mut self,
        plugin_id: &str,
        event: &PluginEvent,
    ) -> Result<PluginResponse> {
        debug!("Dispatching event to plugin '{}': {:?}", plugin_id, event);

        // Check if plugin is enabled
        if !self.is_plugin_enabled(plugin_id) {
            return Err(PluginError::ExecutionError(format!(
                "Plugin '{}' is disabled",
                plugin_id
            )));
        }

        let instance_arc = {
            let plugins = self.plugins.read();
            plugins
                .get(plugin_id)
                .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?
                .instance
                .clone()
        };

        // Acquire instance lock
        let mut instance = instance_arc.lock();
        instance.on_event(event)
    }
}
