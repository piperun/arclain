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
}

/// Process a list of plugin actions
pub fn process_plugin_actions(
    actions: Vec<PluginAction>,
    plugin_id: &str,
    dialog_state: &mut PluginDialogState,
    toaster: &mut Toaster,
    refresh_requests: Option<&Arc<Mutex<Vec<String>>>>,
    lightbox_signal: Option<&Signal<LightboxState>>,
) {
    let ctx = ActionContext {
        lightbox_signal,
        page_display_name_signal: None,
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
