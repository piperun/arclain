pub mod facade_sessions;
mod ui_jobs;

pub use crate::features::plugins::domain::types::RequestId;
pub use facade_sessions::{
    document_buttons, document_is_empty, AppliedUpdate, PluginDocumentButton, PluginNavigation,
    PluginSessions, PluginSlot, SlotView,
};
pub use ui_jobs::{PluginUiFailureContext, PluginUiJobs, PluginUiRequest, PluginUiResult};

use crate::features::plugins::domain::types::{PluginsListState, SnapshotStatus};
use crate::features::plugins::PluginsFeature;
use crate::shared::SharedState;

pub fn request_plugin_snapshot(shared: &SharedState, state: &mut PluginsListState) {
    if state.snapshot_status != SnapshotStatus::Idle {
        return;
    }
    if let Some(snapshot) = shared.plugin_ui_jobs.plugin_snapshot() {
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
    let request_id = shared.plugin_ui_jobs.request(PluginUiRequest::Snapshot);
    state.snapshot_status = SnapshotStatus::Pending;
    state.snapshot_request_id = Some(request_id);
}

pub fn process_plugin_ui_results(shared: &SharedState, plugins: &mut PluginsFeature) {
    for result in shared.plugin_ui_jobs.drain() {
        match result {
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
            PluginUiResult::MutationFinished { result, .. } => {
                // `SetEnabled` no longer runs through this queue -- the
                // plugin detail view calls `ArclainApp::set_plugin_enabled`
                // directly (durable: it persists `enabled_plugins` itself).
                // `Install` is the only operation represented by the generic
                // mutation result; domain approval has its own result so it
                // can invalidate only that plugin's whitelist cache.
                match result {
                    Ok(()) => {
                        plugins.list_state.invalidate_snapshot();
                        plugins.settings_list_state.invalidate_snapshot();
                        shared.plugin_ui_jobs.invalidate_plugin_snapshots();
                        shared.plugin_ui_jobs.invalidate_chrome_snapshot();
                    }
                    Err(error) => shared.toaster.lock().error(error),
                }
            }
            PluginUiResult::DomainApprovalFinished { result, .. } => {
                if let Err(error) = result {
                    shared.toaster.lock().error(error);
                }
            }
            PluginUiResult::ChromeSnapshotLoaded { .. }
            | PluginUiResult::NetworkLogLoaded { .. }
            | PluginUiResult::DomainWhitelistLoaded { .. } => {}
            PluginUiResult::Failed {
                request_id,
                context,
                error,
            } => {
                match context {
                    PluginUiFailureContext::Snapshot => {
                        plugins
                            .list_state
                            .apply_snapshot_failure(request_id, error.clone());
                        plugins
                            .settings_list_state
                            .apply_snapshot_failure(request_id, error.clone());
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
