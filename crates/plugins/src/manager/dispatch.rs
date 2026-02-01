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
            let plugin_ids: Vec<String> = plugins.read().keys().cloned().collect();

            for plugin_id in plugin_ids {
                // Check if plugin is enabled
                let is_enabled = enabled_plugins
                    .read()
                    .get(&plugin_id)
                    .copied()
                    .unwrap_or(false);

                if !is_enabled {
                    continue;
                }

                // Get instance handle
                let instance_arc = {
                    let map = plugins.read();
                    if let Some(p) = map.get(&plugin_id) {
                        p.instance.clone()
                    } else {
                        continue;
                    }
                };

                // Call plugin
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

            let plugin_ids: Vec<String> = plugins.read().keys().cloned().collect();

            for plugin_id in plugin_ids {
                // Check if plugin is enabled
                let is_enabled = enabled_plugins
                    .read()
                    .get(&plugin_id)
                    .copied()
                    .unwrap_or(false);

                if !is_enabled {
                    continue;
                }

                // Get instance handle
                let instance_arc = {
                    let map = plugins.read();
                    if let Some(p) = map.get(&plugin_id) {
                        p.instance.clone()
                    } else {
                        continue;
                    }
                };

                // Call plugin
                let mut instance = instance_arc.lock();

                // Match event to set context and dispatch
                match &event {
                    PluginEvent::OnArchiveOpen { path, password, .. } => {
                        // Set context first (on background thread!)
                        instance.set_archive_context(Some(path.clone()), password.clone());

                        // Then dispatch event
                        let id = "event:archive_opened".to_string();
                        let value = Some(path.clone());

                        if let Err(e) = instance.send_ui_event(&id, value) {
                            error!("Event worker error for {}: {:?}", plugin_id, e);
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

        let mut responses = Vec::new();
        // Only need read lock now since instances are internally locked
        let plugin_ids: Vec<String> = self.plugins.read().keys().cloned().collect();

        for plugin_id in plugin_ids {
            // Check if plugin is enabled
            if !self.is_plugin_enabled(&plugin_id) {
                continue;
            }

            // Get read access to plugins map and clone Arc
            let instance_arc = {
                let plugins = self.plugins.read();
                if let Some(plugin) = plugins.get(&plugin_id) {
                    plugin.instance.clone()
                } else {
                    continue;
                }
            };

            // Acquire instance lock
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
