//! Plugin action handler
//!
//! Processes `PluginAction` values returned from plugin UI events
//! and dispatches them to the appropriate subsystems.

use arclain_plugins::types::{PluginAction, ToastLevel as PluginToastLevel};
use arclain_widgets::{Toast, ToastLevel, Toaster};
use parking_lot::Mutex;
use std::sync::Arc;

use super::dialog_state::PluginDialogState;

/// Process a list of plugin actions
pub fn process_plugin_actions(
    actions: Vec<PluginAction>,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
) {
    for action in actions {
        process_action(action, plugin_id, dialog_state, toaster, refresh_requests);
    }
}

/// Process a single plugin action
fn process_action(
    action: PluginAction,
    plugin_id: &str,
    _dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
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
    }
}

/// Create a callback handler for plugin dialog events
pub fn create_dialog_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
) -> Box<dyn FnMut(&str, Option<String>)> {
    let dialog_state_arc = shared.plugin_dialog_state.clone();
    let toaster_arc = shared.toaster.clone();
    let plugin_manager_arc = shared.services.plugin_manager.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        // Check for close dialog signal
        if element_id == "__dialog_close" {
            dialog_state_arc.lock().close_dialog();
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
                let mut ds = dialog_state_arc.lock();
                // Invalidate layout cache so next frame fetches fresh layout
                ds.invalidate_dialog_layout();
                for action in actions {
                    process_action(
                        action,
                        &pid,
                        &mut ds,
                        &mut toaster,
                        None, // No refresh requests for dialog callbacks
                    );
                }
            }
        }
    })
}

/// Create a callback handler for plugin page events
pub fn create_page_callback(
    shared: &crate::shared::SharedState,
    plugin_id: String,
) -> Box<dyn FnMut(&str, Option<String>)> {
    let dialog_state_arc = shared.plugin_dialog_state.clone();
    let toaster_arc = shared.toaster.clone();
    let plugin_manager_arc = shared.services.plugin_manager.clone();
    let pid = plugin_id;

    Box::new(move |element_id: &str, value: Option<String>| {
        // Check for close page signal
        if element_id == "__page_close" {
            dialog_state_arc.lock().close_page();
            return;
        }

        // Check for open page signal (nested navigation)
        if element_id.starts_with("__page_open:") {
            let new_page_id = element_id.trim_start_matches("__page_open:").to_string();
            dialog_state_arc.lock().open_page(&pid, &new_page_id);
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
                let mut ds = dialog_state_arc.lock();
                // Invalidate layout cache so next frame fetches fresh layout
                ds.invalidate_page_layout();
                for action in actions {
                    process_action(
                        action,
                        &pid,
                        &mut ds,
                        &mut toaster,
                        None, // No refresh requests for page callbacks
                    );
                }
            }
        }
    })
}
