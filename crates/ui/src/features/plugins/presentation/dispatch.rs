//! Origin-aware plugin UI-event dispatch.
//!
//! Events are queued on [`PluginUiJobs`](crate::features::plugins::application::PluginUiJobs)
//! so WASM execution, settings persistence, action routing, ordering, and failures all
//! share one completion path.

use crate::core::tabs::TabId;
use crate::features::plugins::application::PluginUiRequest;
use crate::shared::SharedState;

/// Queue an event for the tab that is active at dispatch time.
pub fn dispatch_plugin_event(
    shared: &SharedState,
    plugin_id: String,
    event_id: String,
    value: Option<String>,
) {
    let origin_tab = shared.signals().tabs.get().active_id();
    dispatch_plugin_event_for_tab(shared, origin_tab, plugin_id, event_id, value);
}

/// Queue an event for an explicitly captured origin tab.
pub fn dispatch_plugin_event_for_tab(
    shared: &SharedState,
    origin_tab: TabId,
    plugin_id: String,
    event_id: String,
    value: Option<String>,
) {
    shared.plugin_ui_jobs.request(PluginUiRequest::UiEvent {
        plugin_id,
        event_id,
        value,
        origin_tab,
    });
}
