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
    }
}
