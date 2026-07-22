mod ui_jobs;

pub use crate::features::plugins::domain::types::RequestId;
pub use ui_jobs::{PluginUiJobs, PluginUiRequest, PluginUiResult, PluginUiTarget};

use crate::features::plugins::domain::types::{PluginsListState, SnapshotStatus};
use crate::features::plugins::presentation::controllers::plugin_controller::{
    process_action, ActionContext,
};
use crate::features::plugins::PluginsFeature;
use crate::shared::SharedState;

pub fn request_plugin_snapshot(shared: &SharedState, state: &mut PluginsListState) {
    if state.snapshot_status != SnapshotStatus::Idle {
        return;
    }
    let user_config = shared.signals().user_config.get();
    if let Some(snapshot) = shared.plugin_ui_jobs.plugin_snapshot(&user_config) {
        state.plugins = snapshot.as_ref().clone();
        state.snapshot_status = SnapshotStatus::Ready;
        state.snapshot_request_id = None;
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
            } => {
                let dialog_signal = shared.signals().plugin_dialog_state.clone();
                let mut dialog_state = dialog_signal.get();
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
                };
                match actions {
                    Ok(actions) => {
                        for action in actions {
                            process_action(
                                action,
                                &plugin_id,
                                &mut dialog_state,
                                &mut toaster,
                                Some(&shared.refresh_requests),
                                &context,
                            );
                        }
                    }
                    Err(error) => toaster.error(error),
                }
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
                let mutation = shared.plugin_ui_jobs.take_mutation(request_id);
                match result {
                    Ok(()) => {
                        if let Some(ui_jobs::PluginUiMutation::SetEnabled { plugin_id, enabled }) =
                            mutation
                        {
                            let mut app = shared.app_state.lock();
                            let mut enabled_plugins = app.user_config.get_enabled_plugins();
                            if enabled {
                                if !enabled_plugins.contains(&plugin_id) {
                                    enabled_plugins.push(plugin_id);
                                }
                            } else {
                                enabled_plugins.retain(|id| id != &plugin_id);
                            }
                            app.user_config.set_enabled_plugins(&enabled_plugins);
                            if let Some(service) = &shared.services.config_service {
                                if let Err(error) = service.save_user_config(&app.user_config) {
                                    tracing::error!(
                                        "Failed to persist plugin enabled state: {error}"
                                    );
                                }
                            }
                            shared.signals().user_config.set(app.user_config.clone());
                        }
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
            PluginUiResult::Failed { error, .. } => shared.toaster.lock().error(error),
        }
    }

    // Keep cheap chrome data warm. The pending-key set coalesces this
    // per frame until the worker result is drained.
    let _ = shared.plugin_ui_jobs.chrome_snapshot();
}
