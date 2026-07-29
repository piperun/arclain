pub mod facade_sessions;
mod ui_jobs;

pub use crate::features::plugins::domain::types::RequestId;
pub use facade_sessions::{
    document_is_empty, AppliedUpdate, PluginNavigation, PluginSessions, PluginSlot, SlotView,
};
pub use ui_jobs::{
    PluginUiFailureContext, PluginUiJobs, PluginUiRequest, PluginUiResult, PluginUiTarget,
};

use crate::features::plugins::domain::types::{PluginsListState, SnapshotStatus};
use crate::features::plugins::presentation::controllers::plugin_controller::{
    process_plugin_actions_with_limit_status, ActionContext,
};
use crate::features::plugins::PluginsFeature;
use crate::shared::SharedState;

pub fn request_plugin_snapshot(shared: &SharedState, state: &mut PluginsListState) {
    if state.snapshot_status != SnapshotStatus::Idle {
        return;
    }
    let user_config = shared.signals().user_config.get();
    if let Some(snapshot) = shared.plugin_ui_jobs.plugin_snapshot(&user_config) {
        match snapshot {
            Ok(snapshot) => {
                state.plugins = snapshot.as_ref().clone();
                state.snapshot_status = SnapshotStatus::Ready;
                state.snapshot_request_id = None;
            }
            Err(error) => {
                state.snapshot_status = SnapshotStatus::Failed(error.to_string());
                state.snapshot_request_id = None;
            }
        }
        return;
    }
    let request_id = shared
        .plugin_ui_jobs
        .request(PluginUiRequest::Snapshot { user_config });
    state.snapshot_status = SnapshotStatus::Pending;
    state.snapshot_request_id = Some(request_id);
}

pub fn process_plugin_ui_results(shared: &SharedState, plugins: &mut PluginsFeature) {
    for result in shared.plugin_ui_jobs.drain() {
        match result {
            PluginUiResult::PageInitialized {
                request_id,
                plugin_id,
                page_id,
                origin_tab,
                actions,
                actions_limited,
            } => {
                let dialog_signal = shared.signals().plugin_dialog_state.clone();
                let mut dialog_state = dialog_signal.get();
                let actions = match actions {
                    Ok(actions) => actions,
                    Err(error) => {
                        if dialog_state.apply_page_init_failure(request_id, error.clone()) {
                            dialog_signal.set(dialog_state);
                            shared.toaster.lock().error(error);
                        }
                        continue;
                    }
                };
                if !dialog_state.apply_page_initialized(request_id) {
                    continue;
                }
                shared.plugin_ui_jobs.invalidate_layout(
                    &plugin_id,
                    &PluginUiTarget::Page(page_id),
                    Some(origin_tab),
                );
                dialog_state.cached_page_layout = None;
                dialog_state.cached_page_layout_stale = false;

                let tabs = shared.signals().tabs.get();
                let Some(origin) = tabs.get(origin_tab).cloned() else {
                    dialog_signal.set(dialog_state);
                    continue;
                };
                drop(tabs);

                let mut toaster = shared.toaster.lock();
                let context = ActionContext {
                    lightbox_signal: Some(&origin.lightbox_state),
                    page_display_name_signal: Some(&origin.page_display_name),
                    metadata_signal: Some(&origin.metadata),
                    shared_state: Some(shared),
                    origin_tab: Some(origin_tab),
                };
                process_plugin_actions_with_limit_status(
                    actions,
                    actions_limited,
                    &plugin_id,
                    &mut dialog_state,
                    &mut toaster,
                    Some(&shared.refresh_requests),
                    &context,
                );
                dialog_signal.set(dialog_state);
            }
            PluginUiResult::SnapshotLoaded {
                request_id,
                plugins: snapshot,
            } => {
                plugins
                    .list_state
                    .apply_snapshot(request_id, snapshot.clone());
                plugins
                    .settings_list_state
                    .apply_snapshot(request_id, snapshot);
            }
            PluginUiResult::MutationFinished { request_id, result } => {
                // `SetEnabled` no longer runs through this queue -- the
                // plugin detail view calls `ArclainApp::set_plugin_enabled`
                // directly (durable: it persists `enabled_plugins` itself).
                // `Install` is this queue's one remaining mutation kind;
                // `take_mutation` is still called unconditionally so its
                // entry in `completed_mutations` never leaks for a request
                // this branch does not otherwise inspect.
                let _ = shared.plugin_ui_jobs.take_mutation(request_id);
                match result {
                    Ok(()) => {
                        plugins.list_state.invalidate_snapshot();
                        plugins.settings_list_state.invalidate_snapshot();
                        shared.plugin_ui_jobs.invalidate_plugin_snapshots();
                        shared.plugin_ui_jobs.invalidate_chrome_snapshot();
                        shared.plugin_ui_jobs.invalidate_all_layouts();
                    }
                    Err(error) => shared.toaster.lock().error(error),
                }
            }
            PluginUiResult::LayoutLoaded { .. }
            | PluginUiResult::ChromeSnapshotLoaded { .. }
            | PluginUiResult::NetworkLogLoaded { .. } => {}
            PluginUiResult::UiEventFinished {
                plugin_id,
                origin_tab,
                result,
                ..
            } => match result {
                Ok(completion) => {
                    if let Some(settings) = completion.settings {
                        persist_plugin_settings(shared, &plugin_id, settings);
                    }
                    process_actions_for_origin(
                        shared,
                        &plugin_id,
                        origin_tab,
                        completion.actions,
                        completion.actions_limited,
                    );
                }
                Err(error) => shared.toaster.lock().error(error),
            },
            PluginUiResult::Failed {
                request_id,
                context,
                error,
            } => {
                match context {
                    PluginUiFailureContext::Snapshot { .. } => {
                        plugins
                            .list_state
                            .apply_snapshot_failure(request_id, error.clone());
                        plugins
                            .settings_list_state
                            .apply_snapshot_failure(request_id, error.clone());
                    }
                    PluginUiFailureContext::PageInit { .. } => {
                        let signal = shared.signals().plugin_dialog_state.clone();
                        let mut state = signal.get();
                        if state.apply_page_init_failure(request_id, error.clone()) {
                            signal.set(state);
                        }
                    }
                    _ => {}
                }
                shared.toaster.lock().error(error);
            }
        }
    }

    // Keep cheap chrome data warm. The pending-key set coalesces this
    // per frame until the worker result is drained.
    let _ = shared.plugin_ui_jobs.chrome_snapshot();
}

fn persist_plugin_settings(
    shared: &SharedState,
    plugin_id: &str,
    settings: std::collections::HashMap<String, String>,
) {
    let Some(facade) = shared.facade.as_ref() else {
        tracing::error!("Failed to save plugin settings: application facade is unavailable");
        shared
            .toaster
            .lock()
            .error("Plugin settings were not saved: application facade is unavailable");
        return;
    };
    let result = shared
        .services
        .tokio_runtime
        .block_on(facade.set_plugin_settings(plugin_id.to_string(), settings));
    match result {
        Ok(()) => {
            let mut app = shared.app_state.lock();
            if let Err(error) = app.refresh_settings_from_facade(facade) {
                tracing::error!(
                    "Failed to refresh settings mirror after plugin settings save: {error}"
                );
            }
        }
        Err(error) => {
            tracing::error!("Failed to save plugin settings: {error:?}");
            shared.toaster.lock().error(error.summary);
        }
    }
}

fn process_actions_for_origin(
    shared: &SharedState,
    plugin_id: &str,
    origin_tab: crate::core::tabs::TabId,
    actions: Vec<arclain_plugins::types::PluginAction>,
    actions_limited: bool,
) {
    let tabs = shared.signals().tabs.get();
    let Some(origin) = tabs.get(origin_tab).cloned() else {
        return;
    };
    drop(tabs);

    let dialog_signal = shared.signals().plugin_dialog_state.clone();
    let mut dialog_state = dialog_signal.get();
    let mut toaster = shared.toaster.lock();
    let context = ActionContext {
        lightbox_signal: Some(&origin.lightbox_state),
        page_display_name_signal: Some(&origin.page_display_name),
        metadata_signal: Some(&origin.metadata),
        shared_state: Some(shared),
        origin_tab: Some(origin_tab),
    };
    process_plugin_actions_with_limit_status(
        actions,
        actions_limited,
        plugin_id,
        &mut dialog_state,
        &mut toaster,
        Some(&shared.refresh_requests),
        &context,
    );
    dialog_signal.set(dialog_state);
}
