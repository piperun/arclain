//! Detached snapshots of enabled plugin instance handles.

use crate::runtime::PluginInstance;
use crate::types::{PluginIdentityKey, TopTabConfig};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::debug;
use wirt::ExecutorRequest;

/// Enabled plugin instance handles detached from [`super::PluginManager`].
///
/// Constructing this snapshot briefly reads the manager's plugin maps and
/// clones each enabled instance `Arc`. Once returned, callers can release any
/// outer `PluginManager` mutex before invoking plugin WASM or reading host
/// state. Aggregate reads deliberately block on every still-current captured
/// instance instead of silently omitting one merely because it is busy. A
/// generation replaced after capture is omitted rather than redirected into
/// the replacement instance.
#[derive(Clone, Default)]
pub struct EnabledPluginSnapshot {
    plugins: Vec<(PluginIdentityKey, String, Arc<Mutex<PluginInstance>>)>,
    executor: Option<Arc<crate::InProcessWirtExecutor>>,
}

impl EnabledPluginSnapshot {
    pub(super) fn new(
        plugins: Vec<(PluginIdentityKey, String, Arc<Mutex<PluginInstance>>)>,
        executor: Arc<crate::InProcessWirtExecutor>,
    ) -> Self {
        Self {
            plugins,
            executor: Some(executor),
        }
    }

    /// Return whether the snapshot captured no enabled plugins.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Return the number of enabled plugins captured by the snapshot.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Iterate over captured plugin IDs without cloning them.
    pub fn plugin_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.plugins.iter().map(|(_, id, _)| id.as_str())
    }

    /// Clone the instance handle for one captured enabled plugin.
    pub fn instance(&self, plugin_id: &str) -> Option<Arc<Mutex<PluginInstance>>> {
        let identity_key = PluginIdentityKey::parse(plugin_id).ok()?;
        self.plugins
            .iter()
            .find(|(key, _, _)| *key == identity_key)
            .map(|(_, _, instance)| instance.clone())
    }

    /// Iterate over captured IDs and cloned instance handles.
    pub fn plugin_instances(
        &self,
    ) -> impl ExactSizeIterator<Item = (&str, Arc<Mutex<PluginInstance>>)> + '_ {
        self.plugins
            .iter()
            .map(|(_, id, instance)| (id.as_str(), instance.clone()))
    }

    /// Read all top tabs from every captured plugin, waiting for busy
    /// instances so the returned snapshot is complete.
    pub fn get_all_top_tabs(&self) -> Vec<(String, TopTabConfig)> {
        let mut all_tabs = Vec::new();

        let Some(executor) = &self.executor else {
            return all_tabs;
        };
        for (_, plugin_id, instance) in &self.plugins {
            let Ok(plugin_id_value) = wirt::PluginId::parse(plugin_id.clone()) else {
                continue;
            };
            match executor
                .execute_for_instance(&plugin_id_value, instance, ExecutorRequest::TopTabs)
                .and_then(wirt::ExecutorResponse::into_top_tabs)
            {
                Ok(tabs) => {
                    all_tabs.extend(tabs.into_iter().map(|tab| (plugin_id.clone(), tab)));
                }
                Err(error) => {
                    debug!("Failed to get top tabs from {}: {:?}", plugin_id, error);
                }
            }
        }

        all_tabs.sort_by(|left, right| left.1.priority.cmp(&right.1.priority));
        all_tabs
    }

    /// Read and chronologically sort network logs from every captured plugin,
    /// waiting for busy instances so the returned snapshot is complete.
    pub fn get_network_log(&self) -> Vec<(SystemTime, String)> {
        let mut all_logs = Vec::new();

        for (_, _, instance) in &self.plugins {
            all_logs.extend(instance.lock().get_network_log());
        }

        all_logs.sort_by(|left, right| left.0.cmp(&right.0));
        all_logs
    }
}
