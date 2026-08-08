use super::epoch::EpochTicker;
use super::quota::{
    call_with_quotas, new_plugin_store, prepare_export_quota, resource_quota_reason,
    validate_actions_result, validate_layout_result, validate_serialized_result,
    validate_top_tabs_result, InstanceAvailability, ResultValidationError,
};
use super::{LoadedComponent, WirtStoreState};
use crate::conversions::{
    convert_plugin_action, convert_plugin_layout, convert_plugin_rule_definition,
    convert_top_tab_config,
};
use crate::{
    PluginAction, PluginError, PluginExtensionPoint, PluginLayout, PluginMetadata,
    PluginRuleDefinition, PluginWorld, Result, TopTabConfig,
};
use std::sync::Arc;
use tracing::{debug, error};
use wasmtime::component::{HasSelf, Linker};
use wasmtime::Store;

impl LoadedComponent {
    pub fn instantiate<H: WirtStoreState>(&self, host: H) -> Result<PluginInstance<H>> {
        let mut store = new_plugin_store(&self.engine, host)?;
        let mut linker = Linker::new(&self.engine);

        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| PluginError::InitError(error.to_string()))?;
        PluginWorld::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut H| state)
            .map_err(|error| PluginError::InitError(error.to_string()))?;

        prepare_export_quota(&mut store).map_err(|_| {
            PluginError::Unavailable("plugin execution quota unavailable".to_string())
        })?;
        let plugin =
            PluginWorld::instantiate(&mut store, &self.component, &linker).map_err(|error| {
                match resource_quota_reason(&error) {
                    Some(reason) => PluginError::Unavailable(reason.to_string()),
                    None => PluginError::InitError(error.to_string()),
                }
            })?;

        debug!("Plugin instance created");
        Ok(PluginInstance {
            store,
            plugin,
            metadata: None,
            availability: InstanceAvailability::default(),
            _epoch_ticker: self.epoch_ticker.clone(),
        })
    }
}

/// Instantiated plugin paired with its caller-supplied host state.
pub struct PluginInstance<H: WirtStoreState> {
    pub(super) store: Store<H>,
    pub(super) plugin: PluginWorld,
    metadata: Option<PluginMetadata>,
    pub(super) availability: InstanceAvailability,
    _epoch_ticker: Arc<EpochTicker>,
}

impl<H: WirtStoreState> PluginInstance<H> {
    fn call_export<T>(
        &mut self,
        call: impl FnOnce(&PluginWorld, &mut Store<H>) -> wasmtime::Result<T>,
        validate: impl FnOnce(&T) -> std::result::Result<(), ResultValidationError>,
        map_ordinary_error: impl FnOnce(String) -> PluginError,
    ) -> Result<T> {
        let Self {
            store,
            plugin,
            availability,
            ..
        } = self;
        call_with_quotas(
            store,
            availability,
            |store| call(plugin, store),
            validate,
            map_ordinary_error,
        )
    }

    pub fn host_state(&self) -> &H {
        self.store.data()
    }

    /// Mutably borrow the host state through this instance.
    ///
    /// Guest exports require their own mutable borrow of `self`, so a host
    /// state borrow cannot be retained across a guest call.
    pub fn host_state_mut(&mut self) -> &mut H {
        self.store.data_mut()
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        self.availability.reason()
    }

    pub fn init(&mut self) -> Result<()> {
        self.call_export(
            |plugin, store| plugin.call_init(store),
            |_| Ok(()),
            PluginError::InitError,
        )?;
        debug!("Plugin initialized successfully");
        Ok(())
    }

    pub fn get_metadata(&mut self) -> Result<PluginMetadata> {
        self.availability.ensure_available()?;
        if let Some(metadata) = &self.metadata {
            return Ok(metadata.clone());
        }

        let metadata = self.call_export(
            |plugin, store| {
                plugin
                    .call_get_metadata(store)
                    .map(|metadata| PluginMetadata {
                        id: metadata.id,
                        name: metadata.name,
                        version: metadata.version,
                        author: metadata.author,
                        description: metadata.description,
                    })
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )?;
        self.metadata = Some(metadata.clone());
        Ok(metadata)
    }

    pub fn get_default_rules(&mut self) -> Result<Vec<PluginRuleDefinition>> {
        self.call_export(
            |plugin, store| {
                plugin.call_get_default_rules(store).map(|rules| {
                    rules
                        .into_iter()
                        .map(convert_plugin_rule_definition)
                        .collect()
                })
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )
    }

    pub fn get_ui_layout(
        &mut self,
        extension_point: &PluginExtensionPoint,
    ) -> Result<PluginLayout> {
        let extension_point = match extension_point {
            PluginExtensionPoint::MainPage => "MainPage".to_string(),
            PluginExtensionPoint::PluginButton => "PluginButton".to_string(),
            PluginExtensionPoint::Panel => "Panel".to_string(),
            PluginExtensionPoint::Dialog(id) => format!("Dialog:{id}"),
            PluginExtensionPoint::Page(id) => format!("Page:{id}"),
        };

        self.call_export(
            |plugin, store| {
                plugin
                    .call_get_ui_layout(store, &extension_point)
                    .map(convert_plugin_layout)
            },
            validate_layout_result,
            PluginError::ExecutionError,
        )
    }

    pub fn send_ui_event(
        &mut self,
        element_id: &str,
        value: Option<String>,
    ) -> Result<Vec<PluginAction>> {
        debug!("Calling plugin UI handler for {element_id}");
        let actions = self.call_export(
            |plugin, store| {
                plugin
                    .call_on_ui_event(store, element_id, value.as_deref())
                    .map(|actions| {
                        actions
                            .into_iter()
                            .map(convert_plugin_action)
                            .collect::<Vec<_>>()
                    })
            },
            |actions| validate_actions_result(actions),
            |message| {
                error!("Failed to call plugin UI handler: {message}");
                PluginError::ExecutionError(message)
            },
        )?;
        debug!("Plugin returned {} actions", actions.len());
        Ok(actions)
    }

    pub fn get_top_tabs(&mut self) -> Result<Vec<TopTabConfig>> {
        self.call_export(
            |plugin, store| {
                plugin.call_get_top_tabs(store).map(|tabs| {
                    tabs.into_iter()
                        .map(convert_top_tab_config)
                        .collect::<Vec<_>>()
                })
            },
            |tabs| validate_top_tabs_result(tabs),
            PluginError::ExecutionError,
        )
    }

    pub fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
}
