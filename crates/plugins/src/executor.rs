//! In-process implementation of Wirt's serializable execution boundary.

use crate::manager::types::ManagedPlugin;
use crate::runtime::PluginInstance;
use crate::types::{PluginError, PluginIdentityKey, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wirt::{
    ExecutorRequest, ExecutorResponse, PluginId, ValidatedExecutorRequest, WirtExecutorBackend,
};

/// Executes Wirt messages against the manager's in-process instance registry.
///
/// This adapter owns only registry and instance references. Product services
/// remain in each instance's host state and never enter the serializable
/// request or response values.
pub struct InProcessWirtExecutor {
    plugins: Arc<RwLock<HashMap<PluginIdentityKey, ManagedPlugin>>>,
    enabled_plugins: Arc<RwLock<HashMap<PluginIdentityKey, bool>>>,
    plugin_state_transition: Arc<Mutex<()>>,
    quarantine: Arc<crate::QuarantineLedger>,
    cached_top_tabs: Arc<Mutex<Option<Vec<(String, crate::types::TopTabConfig)>>>>,
    cached_top_tabs_epoch: Arc<AtomicU64>,
    #[cfg(test)]
    admitted_execution_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    event_dispatch_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    resource_disabled_before_ledger_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl InProcessWirtExecutor {
    pub(crate) fn new(
        plugins: Arc<RwLock<HashMap<PluginIdentityKey, ManagedPlugin>>>,
        enabled_plugins: Arc<RwLock<HashMap<PluginIdentityKey, bool>>>,
        plugin_state_transition: Arc<Mutex<()>>,
        quarantine: Arc<crate::QuarantineLedger>,
        cached_top_tabs: Arc<Mutex<Option<Vec<(String, crate::types::TopTabConfig)>>>>,
        cached_top_tabs_epoch: Arc<AtomicU64>,
    ) -> Self {
        Self {
            plugins,
            enabled_plugins,
            plugin_state_transition,
            quarantine,
            cached_top_tabs,
            cached_top_tabs_epoch,
            #[cfg(test)]
            admitted_execution_hook: Mutex::new(None),
            #[cfg(test)]
            event_dispatch_hook: Mutex::new(None),
            #[cfg(test)]
            resource_disabled_before_ledger_hook: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_admitted_execution_hook(&self, hook: Box<dyn FnOnce() + Send>) {
        *self.admitted_execution_hook.lock() = Some(hook);
    }

    #[cfg(test)]
    pub(crate) fn set_event_dispatch_hook(&self, hook: Box<dyn FnOnce() + Send>) {
        *self.event_dispatch_hook.lock() = Some(hook);
    }

    #[cfg(test)]
    pub(crate) fn set_resource_disabled_before_ledger_hook(&self, hook: Box<dyn FnOnce() + Send>) {
        *self.resource_disabled_before_ledger_hook.lock() = Some(hook);
    }

    fn with_current_instance<T>(
        &self,
        plugin_id: &PluginId,
        expected: Option<&Arc<Mutex<PluginInstance>>>,
        operation: impl FnOnce(&mut PluginInstance) -> Result<T>,
    ) -> Result<T> {
        let plugins = self.plugins.read();
        let managed = plugins
            .get(&plugin_id.identity_key())
            .ok_or_else(|| PluginError::NotFound(plugin_id.as_str().to_string()))?;
        if expected.is_some_and(|expected| !Arc::ptr_eq(expected, &managed.instance)) {
            return Err(PluginError::NotFound(plugin_id.as_str().to_string()));
        }

        // Keep registry membership stable until this exact generation's
        // instance lock is acquired. Unload/reload must take the registry
        // write lock first, so Cleanup cannot finish and then be followed by
        // a delayed guest call that already cloned the old Arc.
        let instance_handle = managed.instance.clone();
        let admission = managed.execution_admission.clone();
        let fingerprint = managed.fingerprint.clone();
        let retry_authorized = managed.retry_authorized;
        let mut instance = instance_handle.lock();
        let Some(_permit) = admission.admit() else {
            return Err(PluginError::Unavailable("plugin is disabled".to_string()));
        };

        #[cfg(test)]
        let hook = self.admitted_execution_hook.lock().take();
        #[cfg(test)]
        if let Some(hook) = hook {
            hook();
        }

        let result = operation(&mut instance);
        let resource_reason = match &result {
            Err(PluginError::ResourceLimit { reason }) => Some(reason.clone()),
            _ => None,
        };
        drop(instance);

        if let Some(reason) = resource_reason {
            drop(plugins);
            let _transition = self.plugin_state_transition.lock();
            admission.disable();
            let identity_key = plugin_id.identity_key();
            let still_current = self
                .plugins
                .read()
                .get(&identity_key)
                .is_some_and(|managed| Arc::ptr_eq(&managed.instance, &instance_handle));
            if still_current {
                self.enabled_plugins.write().insert(identity_key, false);
            }
            #[cfg(test)]
            if let Some(hook) = self.resource_disabled_before_ledger_hook.lock().take() {
                hook();
            }
            self.cached_top_tabs_epoch.fetch_add(1, Ordering::AcqRel);
            *self.cached_top_tabs.lock() = None;
            if retry_authorized {
                self.quarantine.record_failed_retry(&fingerprint, &reason)?;
            } else {
                self.quarantine
                    .record_initial_violation(&fingerprint, &reason)?;
            }
        }

        result
    }

    /// Execute a bounded message against a transient instance that is not yet
    /// published in the manager registry.
    pub(crate) fn execute_transient(
        instance: &mut PluginInstance,
        request: ExecutorRequest,
    ) -> Result<ExecutorResponse> {
        request.validate()?;
        let response = dispatch(instance, request)?;
        response.validate()?;
        Ok(response)
    }

    /// Execute only if the requested plugin still maps to the captured
    /// instance generation.
    pub(crate) fn execute_for_instance(
        &self,
        plugin_id: &PluginId,
        expected: &Arc<Mutex<PluginInstance>>,
        request: ExecutorRequest,
    ) -> Result<ExecutorResponse> {
        request.validate()?;
        let response = self.with_current_instance(plugin_id, Some(expected), |instance| {
            dispatch(instance, request)
        })?;
        response.validate()?;
        Ok(response)
    }

    /// Admit one host-owned follow-up effect only while the captured plugin
    /// generation is still registered and enabled. The returned guard keeps
    /// disable, unload, and reload from completing until that effect ends.
    pub(crate) fn admit_host_effect_for_instance(
        &self,
        plugin_id: &PluginId,
        expected: &Arc<Mutex<PluginInstance>>,
    ) -> Option<crate::manager::types::ExecutionPermit> {
        let plugins = self.plugins.read();
        let managed = plugins.get(&plugin_id.identity_key())?;
        if !Arc::ptr_eq(expected, &managed.instance) {
            return None;
        }
        managed.execution_admission.admit()
    }

    /// Execute against one captured generation while a host-owned archive
    /// event context is installed.
    ///
    /// The context remains inside `HostFunctions`; only `request` crosses the
    /// Wirt message boundary.
    pub(crate) fn execute_with_event_context(
        &self,
        plugin_id: &PluginId,
        expected: &Arc<Mutex<PluginInstance>>,
        request: ExecutorRequest,
        event_context: crate::host_functions::EventContext,
    ) -> Result<ExecutorResponse> {
        request.validate()?;
        let response = self.with_current_instance(plugin_id, Some(expected), |instance| {
            instance.set_event_context(Some(event_context));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                let hook = self.event_dispatch_hook.lock().take();
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook();
                }
                dispatch(instance, request)
            }));
            instance.set_event_context(None);
            match result {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            }
        })?;
        response.validate()?;
        Ok(response)
    }
}

impl WirtExecutorBackend for InProcessWirtExecutor {
    fn execute_validated(
        &self,
        plugin_id: &PluginId,
        request: ValidatedExecutorRequest,
    ) -> Result<ExecutorResponse> {
        self.with_current_instance(plugin_id, None, |instance| {
            dispatch(instance, request.into_inner())
        })
    }
}

fn dispatch(instance: &mut PluginInstance, request: ExecutorRequest) -> Result<ExecutorResponse> {
    match request {
        ExecutorRequest::Init => {
            instance.init()?;
            Ok(ExecutorResponse::Empty)
        }
        ExecutorRequest::Metadata => instance.get_metadata().map(ExecutorResponse::Metadata),
        ExecutorRequest::DefaultRules => instance
            .get_default_rule_definitions()
            .map(ExecutorResponse::Rules),
        ExecutorRequest::UiLayout { extension_point } => instance
            .get_ui_layout(extension_point)
            .map(ExecutorResponse::Layout),
        ExecutorRequest::UiEvent { id, value } => instance
            .send_ui_event(&id, value)
            .map(ExecutorResponse::Actions),
        ExecutorRequest::TopTabs => instance.get_top_tabs().map(ExecutorResponse::TopTabs),
        ExecutorRequest::Cleanup => {
            instance.cleanup()?;
            Ok(ExecutorResponse::Empty)
        }
    }
}
