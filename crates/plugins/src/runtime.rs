//! WASM runtime wrapper using wasmtime component model
//!
//! This module provides the WASM runtime with full host function support.

use crate::conversions::{
    convert_plugin_action, convert_plugin_layout, convert_plugin_rule_definition,
    convert_top_tab_config,
};
use crate::host_functions::HostFunctions;
use crate::types::{PluginCapability, PluginError, PluginExtensionPoint, PluginMetadata, Result};
use wirt::PluginWorld;

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

const FUEL_PER_EXPORT: u64 = 10_000_000;
// The epoch pair below is a liveness dead-man switch, NOT a work budget:
// `FUEL_PER_EXPORT` above is the load-bearing bound on how much a plugin
// may compute per export, and it is deterministic regardless of machine
// speed or load. Wall-clock enters only because fuel cannot see time
// spent on the host side of a hostcall: a guest *looping* over cheap
// hostcalls burns almost no fuel while pinning the plugin worker thread
// for as long as the loop keeps running. That loop is what this ceiling
// reaps -- and it is the only thing it can reap. The trap lands at guest
// epoch checks (function entries and loop backedges), so a single
// hostcall that never returns pins the worker at ANY deadline value;
// bounding an individual hostcall is each hostcall implementation's own
// job (the network layer's per-request timeout, etc.), never this
// mechanism's.
//
// Sizing rule: this ceiling must dwarf the slowest *legitimate* export,
// not typical work, because tripping it is catastrophic -- an epoch trap
// (`Trap::Interrupt`) permanently poisons the instance (see
// `resource_quota_reason`), and the shipped product has no recovery path
// (`unavailable_reason` has no production consumer and `reload_plugin`
// no production caller), so a false positive kills the plugin until the
// application restarts. The network layer's per-request contract allows
// a single hostcall `arclain_network::DEFAULT_REQUEST_TIMEOUT` (30s) --
// and that bound covers whole transfers, which resume beyond it via
// ranged requests rather than running longer -- so an export making a
// few sequential requests legitimately runs minutes. The ceiling
// therefore sits at minutes; the sizing test pins its floor.
//
// Do not re-tune fuel and this pair toward each other. When this was 10
// ticks (~100ms), a well-behaved guest that had consumed 2 of its
// 10,000,000 fuel was trapped on wall-clock alone -- one slow hostcall or
// a loaded machine was enough to brick a correct plugin mid-call.
const EPOCH_TICKS_PER_EXPORT: u64 = 30_000;
const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(10);
const MAX_UI_ELEMENTS: usize = 10_000;
const MAX_ACTIONS: usize = 1_024;
const MAX_SERIALIZED_RESULT_BYTES: usize = 1024 * 1024;
// Measured from the largest host-side representations permitted by the current
// WIT result shapes: layout 2,568,576 bytes, top tabs 3,256,216 bytes, rules
// 2,535,880 bytes, and actions 1,155,072 bytes. Eight MiB leaves more than 2x
// headroom without retaining Wasmtime's unsafe 128 MiB default.
const HOSTCALL_FUEL_BYTES: usize = 8 * 1024 * 1024;
// Wasmtime 47.0.1 keeps this error type private, so classification is pinned to
// the exact static root-cause text from component/func/options.rs.
const WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED: &str =
    "too much data is being copied between the host and the guest: fuel allocated for hostcalls has been exhausted";
// Wasmtime's ResourceLimiter count errors are likewise private. Keep these
// locked-version strings exact so unrelated guest errors cannot be mistaken for
// terminal quota failures or leak engine details through PluginError.
const WASMTIME_47_INSTANCE_COUNT_EXCEEDED: &str =
    "resource limit exceeded: instance count too high at 33";
const WASMTIME_47_MEMORY_COUNT_EXCEEDED: &str =
    "resource limit exceeded: memory count too high at 5";
const WASMTIME_47_TABLE_COUNT_EXCEEDED: &str = "resource limit exceeded: table count too high at 9";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaViolation {
    Result,
}

impl QuotaViolation {
    fn redacted_reason(self) -> &'static str {
        match self {
            Self::Result => "plugin result quota exceeded",
        }
    }
}

#[derive(Debug)]
enum ResultValidationError {
    Quota(QuotaViolation),
    Serialization(serde_json::Error),
}

#[derive(Default)]
struct ResultSizeWriter {
    bytes_written: usize,
    exceeded: bool,
}

impl std::io::Write for ResultSizeWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.bytes_written.saturating_add(buffer.len()) > MAX_SERIALIZED_RESULT_BYTES {
            self.exceeded = true;
            return Err(std::io::Error::other("plugin result quota exceeded"));
        }
        self.bytes_written += buffer.len();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_serialized_result<T: Serialize + ?Sized>(
    value: &T,
) -> std::result::Result<(), ResultValidationError> {
    let mut writer = ResultSizeWriter::default();
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        return if writer.exceeded {
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        } else {
            Err(ResultValidationError::Serialization(error))
        };
    }
    Ok(())
}

fn validate_layout_result(
    layout: &crate::types::PluginLayout,
) -> std::result::Result<(), ResultValidationError> {
    fn charge_work(
        total: &mut usize,
        additional: usize,
        limit: usize,
    ) -> std::result::Result<(), ResultValidationError> {
        *total = total
            .checked_add(additional)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if *total > limit {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
        Ok(())
    }

    fn charge_elements(
        roots: &[crate::types::PluginUiElement],
        work: &mut usize,
    ) -> std::result::Result<(), ResultValidationError> {
        use crate::types::PluginUiElement;

        let mut stack = vec![roots.iter()];
        while let Some(element) = stack.last_mut().and_then(Iterator::next) {
            charge_work(work, 1, MAX_UI_ELEMENTS)?;
            match element {
                PluginUiElement::RadioGroup { options, .. }
                | PluginUiElement::Dropdown { options, .. } => {
                    charge_work(work, options.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Tabs { tabs, .. } => {
                    charge_work(work, tabs.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::ListContainer { items, .. } => stack.push(items.iter()),
                PluginUiElement::TagChips { tags, max_display } => {
                    let display_limit = max_display
                        .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX))
                        .unwrap_or(tags.len());
                    let visible = display_limit.min(tags.len());
                    let overflow_label = usize::from(visible < tags.len());
                    charge_work(work, visible, MAX_UI_ELEMENTS)?;
                    charge_work(work, overflow_label, MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Toolbar { buttons } => {
                    charge_work(work, buttons.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::Carousel { images, .. } => {
                    charge_work(work, images.len(), MAX_UI_ELEMENTS)?;
                }
                PluginUiElement::KeyValueList { items, .. }
                | PluginUiElement::MetadataGrid { items, .. } => {
                    charge_work(work, items.len(), MAX_UI_ELEMENTS)?;
                }
                _ => {}
            }

            while stack.last().is_some_and(|elements| elements.len() == 0) {
                stack.pop();
            }
        }
        Ok(())
    }

    let mut work = 0usize;
    match layout {
        crate::types::PluginLayout::Single { elements } => {
            charge_elements(elements, &mut work)?;
        }
        crate::types::PluginLayout::Split {
            sidebar, content, ..
        } => {
            charge_elements(sidebar, &mut work)?;
            charge_elements(content, &mut work)?;
        }
    }

    validate_serialized_result(layout)
}

fn validate_actions_result(
    actions: &[crate::types::PluginAction],
) -> std::result::Result<(), ResultValidationError> {
    let mut work = 0usize;
    for action in actions {
        work = work
            .checked_add(1)
            .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        if let crate::types::PluginAction::OpenLightbox { images, .. } = action {
            work = work
                .checked_add(images.len())
                .ok_or(ResultValidationError::Quota(QuotaViolation::Result))?;
        }
        if work > MAX_ACTIONS {
            return Err(ResultValidationError::Quota(QuotaViolation::Result));
        }
    }
    validate_serialized_result(actions)
}

fn validate_top_tabs_result(
    tabs: &[crate::types::TopTabConfig],
) -> std::result::Result<(), ResultValidationError> {
    if tabs.len() > MAX_UI_ELEMENTS {
        return Err(ResultValidationError::Quota(QuotaViolation::Result));
    }
    validate_serialized_result(tabs)
}

#[derive(Debug, Default)]
struct InstanceAvailability {
    reason: Option<&'static str>,
}

impl InstanceAvailability {
    fn ensure_available(&self) -> Result<()> {
        match self.reason {
            Some(reason) => Err(PluginError::Unavailable(reason.to_string())),
            None => Ok(()),
        }
    }

    fn mark_unavailable<T>(&mut self, reason: &'static str) -> Result<T> {
        self.reason = Some(reason);
        Err(PluginError::Unavailable(reason.to_string()))
    }

    fn reason(&self) -> Option<&'static str> {
        self.reason
    }
}

/// Classifies a wasmtime call failure as *terminal* -- the instance must
/// never be called again -- returning the redacted, host-generated
/// reason to report if so. `None` means the failure is ordinary (a guest
/// error the caller can map to `PluginError::ExecutionError` and move on
/// from; the instance stays usable).
///
/// Every `wasmtime::Trap` variant is terminal, not just `OutOfFuel`/
/// `Interrupt`: a wasmtime *component* instance is permanently poisoned
/// by any trap at all (an out-of-bounds guest panic included) -- a
/// second call into the same `Store` after, say, an unreachable-code
/// trap fails again with wasmtime's own internal "cannot enter component
/// instance" error, not a fresh attempt. Before this covered every
/// variant, only the two quota-shaped traps marked the instance
/// `Unavailable`; a genuine guest panic fell through to the generic
/// `_ => None` arm below, leaving `InstanceAvailability` reporting
/// "available" while the underlying `Store` was already permanently
/// unusable -- so the *next* call would attempt the WASM call again and
/// surface that confusing wasmtime-internal string instead of this
/// crate's own redacted, stable `PluginError::Unavailable` reason.
fn resource_quota_reason(error: &wasmtime::Error) -> Option<&'static str> {
    if let Some(trap) = error.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::OutOfFuel => Some("plugin fuel quota exceeded"),
            wasmtime::Trap::Interrupt => Some("plugin execution deadline exceeded"),
            _ => Some("plugin execution trapped"),
        };
    }

    match error.root_cause().to_string().as_str() {
        WASMTIME_47_HOSTCALL_FUEL_EXHAUSTED => {
            return Some("plugin hostcall data quota exceeded");
        }
        WASMTIME_47_INSTANCE_COUNT_EXCEEDED => {
            return Some("plugin instance quota exceeded");
        }
        WASMTIME_47_MEMORY_COUNT_EXCEEDED => {
            return Some("plugin memory quota exceeded");
        }
        WASMTIME_47_TABLE_COUNT_EXCEEDED => {
            return Some("plugin table quota exceeded");
        }
        _ => {}
    }

    error
        .downcast_ref::<crate::host_functions::StoreQuotaExceeded>()
        .map(|quota| match quota.kind {
            crate::host_functions::StoreQuotaKind::Memory => "plugin memory quota exceeded",
            crate::host_functions::StoreQuotaKind::Table => "plugin table quota exceeded",
        })
}

fn call_with_quotas<T>(
    store: &mut Store<HostFunctions>,
    availability: &mut InstanceAvailability,
    call: impl FnOnce(&mut Store<HostFunctions>) -> wasmtime::Result<T>,
    validate: impl FnOnce(&T) -> std::result::Result<(), ResultValidationError>,
    map_ordinary_error: impl FnOnce(String) -> PluginError,
) -> Result<T> {
    availability.ensure_available()?;
    if prepare_export_quota(store).is_err() {
        return availability.mark_unavailable("plugin execution quota unavailable");
    }

    let value = match call(store) {
        Ok(value) => value,
        Err(error) => {
            if let Some(reason) = resource_quota_reason(&error) {
                return availability.mark_unavailable(reason);
            }
            return Err(map_ordinary_error(error.to_string()));
        }
    };

    match validate(&value) {
        Ok(()) => {}
        Err(ResultValidationError::Quota(violation)) => {
            return availability.mark_unavailable(violation.redacted_reason());
        }
        Err(ResultValidationError::Serialization(error)) => {
            return Err(PluginError::Serialization(error));
        }
    }
    Ok(value)
}

fn prepare_export_quota(store: &mut Store<HostFunctions>) -> wasmtime::Result<()> {
    store.set_fuel(FUEL_PER_EXPORT)?;
    store.set_epoch_deadline(EPOCH_TICKS_PER_EXPORT);
    Ok(())
}

fn new_plugin_store(
    engine: &Engine,
    host_functions: HostFunctions,
) -> Result<Store<HostFunctions>> {
    let mut store = Store::new(engine, host_functions);
    store.set_hostcall_fuel(HOSTCALL_FUEL_BYTES);
    store.limiter(|host| &mut host.store_limiter);
    prepare_export_quota(&mut store)
        .map_err(|_| PluginError::WasmError("failed to configure plugin fuel quota".to_string()))?;
    store.epoch_deadline_trap();
    Ok(store)
}

struct EpochTicker {
    control: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
    #[cfg(test)]
    exited: Arc<std::sync::atomic::AtomicBool>,
}

impl EpochTicker {
    fn spawn(engine: Engine) -> std::io::Result<Self> {
        let control = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let thread_control = control.clone();
        #[cfg(test)]
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(test)]
        let thread_exited = exited.clone();
        let worker = std::thread::Builder::new()
            .name("arclain-wasm-epoch".to_string())
            .spawn(move || {
                let (stop, wake) = &*thread_control;
                let mut stopped = stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    let (next_stop, wait) = wake
                        .wait_timeout(stopped, EPOCH_TICK_INTERVAL)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    stopped = next_stop;
                    if *stopped {
                        break;
                    }
                    if wait.timed_out() {
                        engine.increment_epoch();
                    }
                }
                #[cfg(test)]
                thread_exited.store(true, std::sync::atomic::Ordering::Release);
            })?;

        Ok(Self {
            control,
            worker: Some(worker),
            #[cfg(test)]
            exited,
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        let (stop, wake) = &*self.control;
        *stop.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("WASM epoch ticker terminated unexpectedly");
            }
        }
    }
}

/// WASM runtime for executing plugins
pub struct WasmRuntime {
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
}

impl WasmRuntime {
    /// Create a new WASM runtime
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true); // Enable component model
        config.consume_fuel(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config).map_err(|e| PluginError::WasmError(e.to_string()))?;
        let epoch_ticker = Arc::new(EpochTicker::spawn(engine.clone()).map_err(|_| {
            PluginError::WasmError("failed to start WASM epoch ticker".to_string())
        })?);

        info!("WASM runtime initialized (Component Model enabled)");
        Ok(Self {
            engine,
            epoch_ticker,
        })
    }

    #[cfg(test)]
    fn epoch_ticker_exit_probe(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.epoch_ticker.exited.clone()
    }

    /// Load a WASM component from a file
    pub fn load_module(&self, id: String, path: &Path) -> Result<LoadedPlugin> {
        debug!("Loading WASM component from: {}", path.display());

        let component = Component::from_file(&self.engine, path)
            .map_err(|e| PluginError::LoadError(format!("Failed to load component: {}", e)))?;

        info!("WASM component loaded successfully: {}", path.display());

        Ok(LoadedPlugin {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: path.to_path_buf(),
        })
    }

    /// Load a WASM component from bytes
    pub fn load_module_from_bytes(&self, id: String, bytes: &[u8]) -> Result<LoadedPlugin> {
        debug!("Loading WASM component from bytes ({} bytes)", bytes.len());

        let component = Component::from_binary(&self.engine, bytes)
            .map_err(|e| PluginError::LoadError(e.to_string()))?;

        info!("WASM component loaded successfully from bytes");

        Ok(LoadedPlugin {
            id,
            component,
            engine: self.engine.clone(),
            epoch_ticker: self.epoch_ticker.clone(),
            _path: std::path::PathBuf::from("<bytes>"),
        })
    }
}

/// A loaded WASM plugin ready for execution
pub struct LoadedPlugin {
    pub id: String,
    component: Component,
    engine: Engine,
    epoch_ticker: Arc<EpochTicker>,
    _path: std::path::PathBuf,
}

impl LoadedPlugin {
    /// Instantiate a plugin with its host-function state.
    ///
    /// Pre-bridge this had a `_with_backend` sibling that handed an
    /// `Arc<dyn ArchiveBackend>` to the host so `list_archive_files`
    /// could re-list the archive. That path is gone: the host reads
    /// the active tab's already-listed entries through the
    /// `ActiveTabBridge`, so no plugin needs raw backend access. The
    /// backend param (and the `with_backend` constructors it fed)
    /// were dead after that rewire.
    pub fn instantiate(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
        )?;

        self.instantiate_with_host_functions(host_funcs, active_tab_bridge)
    }

    pub(crate) fn instantiate_with_plugin_log_dir(
        &self,
        capabilities: Vec<PluginCapability>,
        requests_per_minute: u32,
        settings: HashMap<String, String>,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
        plugin_log_dir: &Path,
    ) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new_with_plugin_log_dir(
            self.id.clone(),
            capabilities.into_iter().collect(),
            requests_per_minute,
            settings,
            plugin_log_dir,
        )?;

        self.instantiate_with_host_functions(host_funcs, active_tab_bridge)
    }

    pub(crate) fn instantiate_for_metadata_validation(&self) -> Result<PluginInstance> {
        let host_funcs = HostFunctions::new_for_metadata_validation(self.id.clone())?;
        self.instantiate_with_host_functions(host_funcs, None)
    }

    fn instantiate_with_host_functions(
        &self,
        mut host_funcs: HostFunctions,
        active_tab_bridge: Option<Arc<dyn crate::ActiveTabBridge>>,
    ) -> Result<PluginInstance> {
        host_funcs.active_tab = active_tab_bridge;

        let mut store = new_plugin_store(&self.engine, host_funcs)?;
        let mut linker = Linker::new(&self.engine);

        // Link WASI functions
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Link host functions
        PluginWorld::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostFunctions| state)
            .map_err(|e| PluginError::InitError(e.to_string()))?;

        // Instantiate the component
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

/// An instantiated plugin that can receive events
pub struct PluginInstance {
    store: Store<HostFunctions>,
    plugin: PluginWorld,
    metadata: Option<PluginMetadata>,
    availability: InstanceAvailability,
    _epoch_ticker: Arc<EpochTicker>,
}

impl PluginInstance {
    fn call_export<T>(
        &mut self,
        call: impl FnOnce(&PluginWorld, &mut Store<HostFunctions>) -> wasmtime::Result<T>,
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

    /// Return the host-generated reason this instance was made terminal.
    /// Guest-provided error details are never stored here.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.availability.reason()
    }

    /// Initialize the plugin
    pub fn init(&mut self) -> Result<()> {
        self.call_export(
            |plugin, store| plugin.call_init(store),
            |_| Ok(()),
            PluginError::InitError,
        )?;

        debug!("Plugin initialized successfully");
        Ok(())
    }

    /// Get plugin metadata.
    ///
    /// Cached per-instance after the first WIT call. Plugins built
    /// against the post-2026-05-07 WIT export `get-metadata` and
    /// self-report id/name/version/author/description; the host caches
    /// the result so repeated calls don't re-cross the WASM boundary
    /// (the manager's `install_plugin` flow asks once at install time
    /// to derive a stable plugin id for the manifest it writes).
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

    /// Get the organization rules supplied by this plugin.
    pub fn get_default_rules(&mut self) -> Result<Vec<arclain_core::OrganizationRule>> {
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

    /// Get UI layout for a specific extension point
    pub fn get_ui_layout(
        &mut self,
        extension_point: PluginExtensionPoint,
    ) -> Result<crate::types::PluginLayout> {
        let ep_str = match extension_point {
            PluginExtensionPoint::MainPage => "MainPage".to_string(),
            PluginExtensionPoint::PluginButton => "PluginButton".to_string(),
            PluginExtensionPoint::Panel => "Panel".to_string(),
            PluginExtensionPoint::Dialog(ref id) => format!("Dialog:{}", id),
            PluginExtensionPoint::Page(ref id) => format!("Page:{}", id),
        };

        self.call_export(
            |plugin, store| {
                plugin
                    .call_get_ui_layout(store, &ep_str)
                    .map(convert_plugin_layout)
            },
            validate_layout_result,
            PluginError::ExecutionError,
        )
    }

    /// Send a UI event to the plugin and get actions back
    pub fn send_ui_event(
        &mut self,
        element_id: &str,
        value: Option<String>,
    ) -> Result<Vec<crate::types::PluginAction>> {
        debug!(
            "PluginInstance::send_ui_event: Calling plugin handler for {}",
            element_id
        );

        let actions = self.call_export::<Vec<crate::types::PluginAction>>(
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
                error!(
                    "PluginInstance::send_ui_event: Failed to call plugin: {}",
                    message
                );
                PluginError::ExecutionError(message)
            },
        )?;

        debug!(
            "PluginInstance::send_ui_event: Plugin returned {} actions",
            actions.len()
        );

        Ok(actions)
    }

    /// Clean up the plugin
    pub fn cleanup(&mut self) -> Result<()> {
        // Cleanup is handled by Drop in Component Model usually,
        // or we can add a specific cleanup function to WIT
        Ok(())
    }

    /// Set the content cache for host functions
    pub fn set_content_cache(&mut self, cache: Option<Arc<arclain_data::ContentCache>>) {
        let host = self.store.data_mut();
        match cache {
            Some(c) => host.set_content_cache(c),
            None => host.content_cache = None,
        }
    }

    /// Set the resource manager for host functions
    pub fn set_resource_manager(&mut self, manager: Option<Arc<arclain_data::ResourceManager>>) {
        let host = self.store.data_mut();
        match manager {
            Some(m) => host.set_resource_manager(m),
            None => host.resource_manager = None,
        }
    }

    /// Install the bridge to the host's per-tab signal tree for this
    /// instance. Replaces the pre-bridge `set_metadata_signal` +
    /// `set_archive_context` pair — see `crate::active_tab`.
    pub fn set_active_tab_bridge(&mut self, bridge: Arc<dyn crate::ActiveTabBridge>) {
        self.store.data_mut().set_active_tab_bridge(bridge);
    }

    /// Install (or clear) the per-event context for this instance.
    /// The dispatch worker wraps `send_ui_event` with set / clear
    /// calls so host-function reads inside the handler resolve to
    /// the originating tab.
    pub fn set_event_context(&mut self, ctx: Option<crate::host_functions::EventContext>) {
        self.store.data_mut().set_event_context(ctx);
    }

    #[cfg(test)]
    pub(crate) fn has_event_context_for_test(&self) -> bool {
        self.store.data().event_context.is_some()
    }

    /// Set the async HTTP client for host functions
    pub fn set_async_http_client(&mut self, client: Option<Arc<arclain_network::AsyncHttpClient>>) {
        let host = self.store.data_mut();
        match client {
            Some(c) => host.set_async_http_client(c),
            None => host.async_http_client = None,
        }
    }

    /// Set the library service for host functions
    #[cfg(feature = "gameta")]
    pub fn set_library_service(&mut self, lib_svc: Option<Arc<arclain_core::LibraryService>>) {
        let host = self.store.data_mut();
        match lib_svc {
            Some(c) => host.set_library_service(c),
            None => host.library_service = None,
        }
    }

    /// Set the gameta server client for host functions
    pub fn set_gameta_client(
        &mut self,
        client: Option<Arc<arclain_network::features::gameta_client::GametaClient>>,
    ) {
        let host = self.store.data_mut();
        match client {
            Some(c) => host.set_gameta_client(c),
            None => host.gameta_client = None,
        }
    }

    /// Get gameta client reference (if configured)
    pub fn get_gameta_client(
        &self,
    ) -> Option<Arc<arclain_network::features::gameta_client::GametaClient>> {
        let data = self.store.data();
        data.gameta_client.clone()
    }

    pub fn try_acquire_network_host_service(
        &self,
        service_scope: &str,
    ) -> std::result::Result<(), String> {
        let data = self.store.data();
        let client = data
            .async_http_client
            .as_ref()
            .ok_or_else(|| "plugin network policy client unavailable".to_string())?;
        client
            .try_acquire_plugin_host_service(data.plugin_id.as_str(), service_scope)
            .map_err(|error| error.to_string())
    }

    pub fn data_materialization_limit(&self) -> usize {
        self.store.data().data_service.materialization_limit()
    }

    /// Check the immutable manifest capabilities attached to this instance.
    pub fn has_capabilities(&self, required: &[PluginCapability]) -> bool {
        self.store.data().has_capabilities(required)
    }

    /// Get a handle to the active-tab bridge if one has been
    /// installed. Used by the dispatch worker to snapshot per-tab
    /// signals at event-fire time.
    pub fn get_active_tab_bridge(&self) -> Option<Arc<dyn crate::ActiveTabBridge>> {
        let data = self.store.data();
        data.active_tab.clone()
    }

    /// Get network logs from the plugin
    pub fn get_network_log(&self) -> Vec<(std::time::SystemTime, String)> {
        let data = self.store.data();
        let logs = data.network_log.lock();
        logs.clone()
    }

    /// Get current settings from the plugin
    pub fn get_settings(&self) -> Option<std::collections::HashMap<String, String>> {
        let data = self.store.data();
        let settings = data.settings.lock();
        Some(settings.clone())
    }

    /// Cheap clone of the plugin's `settings_dirty` flag — checked by
    /// `PluginManager::get_all_settings` without taking the instance
    /// lock (audit P14).
    pub fn settings_dirty_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.store.data().settings_dirty.clone()
    }

    /// Get top-level tabs registered by this plugin
    pub fn get_top_tabs(&mut self) -> Result<Vec<crate::types::TopTabConfig>> {
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
}

#[cfg(test)]
mod resource_limit_tests {
    use super::*;

    #[cfg(any(windows, target_os = "linux"))]
    const LARGE_LIFT_BYTES: usize = 120 * 1024 * 1024;
    #[cfg(any(windows, target_os = "linux"))]
    const HOSTCALL_MEMORY_CHILD: &str = "ARCLAIN_HOSTCALL_MEMORY_CHILD";
    type ByteListLift = wasmtime::component::TypedFunc<(), (Vec<u8>,)>;

    fn instantiate_byte_list_fixture(
        runtime: &WasmRuntime,
        byte_len: usize,
    ) -> anyhow::Result<(Store<HostFunctions>, ByteListLift)> {
        let memory_pages = (byte_len + 8).div_ceil(64 * 1024);
        let length_bytes = (byte_len as u32).to_le_bytes();
        let encoded_length = length_bytes
            .iter()
            .map(|byte| format!("\\{byte:02x}"))
            .collect::<String>();
        let component = Component::new(
            &runtime.engine,
            format!(
                r#"
                    (component
                        (core module $fixture
                            (memory (export "memory") {memory_pages})
                            (data (i32.const 0) "\08\00\00\00{encoded_length}")
                            (func (export "lift") (result i32)
                                i32.const 0))
                        (core instance $instance (instantiate $fixture))
                        (alias core export $instance "memory" (core memory $memory))
                        (alias core export $instance "lift" (core func $lift))
                        (type $bytes (list u8))
                        (type $lift-type (func (result $bytes)))
                        (func (export "lift") (type $lift-type)
                            (canon lift (core func $lift) (memory $memory))))
                "#,
            ),
        )?;
        let host = HostFunctions::new_for_metadata_validation("hostcall-lift-test".to_string())?;
        let mut store = new_plugin_store(&runtime.engine, host)?;
        let instance = Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
        let lift = instance.get_typed_func::<(), (Vec<u8>,)>(&mut store, "lift")?;
        Ok((store, lift))
    }

    #[cfg(windows)]
    fn current_working_set_bytes() -> usize {
        use std::ffi::c_void;

        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
        }
        #[link(name = "psapi")]
        extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut c_void,
                counters: *mut ProcessMemoryCounters,
                size: u32,
            ) -> i32;
        }

        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        // SAFETY: both functions are called with the current process pseudo-handle
        // and a correctly sized writable PROCESS_MEMORY_COUNTERS buffer.
        let succeeded = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        assert_ne!(succeeded, 0, "GetProcessMemoryInfo must succeed");
        counters.working_set_size
    }

    #[cfg(target_os = "linux")]
    fn current_working_set_bytes() -> usize {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<usize>().ok())
            .expect("/proc/self/status must contain VmRSS");
        rss_kib * 1024
    }

    fn instantiate_resource_fixture(
        runtime: &WasmRuntime,
        core_module: &str,
    ) -> wasmtime::Result<()> {
        let component = Component::new(&runtime.engine, resource_fixture_wat(core_module))?;
        let host = HostFunctions::new_for_metadata_validation("resource-limit-test".to_string())
            .expect("test plugin ID must be valid");
        let mut store =
            new_plugin_store(&runtime.engine, host).expect("test store configuration must succeed");
        let linker = Linker::new(&runtime.engine);
        linker.instantiate(&mut store, &component)?;
        Ok(())
    }

    fn instantiate_core_instance_fixture(
        runtime: &WasmRuntime,
        count: usize,
    ) -> wasmtime::Result<()> {
        let component = Component::new(&runtime.engine, core_instance_fixture_wat(count))?;
        let host = HostFunctions::new_for_metadata_validation("instance-limit-test".to_string())
            .expect("test plugin ID must be valid");
        let mut store =
            new_plugin_store(&runtime.engine, host).expect("test store configuration must succeed");
        Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
        Ok(())
    }

    fn resource_fixture_wat(core_module: &str) -> String {
        format!(
            "(component (core module $fixture {core_module}) (core instance $instance (instantiate $fixture)))"
        )
    }

    fn core_instance_fixture_wat(count: usize) -> String {
        let instances = "(core instance (instantiate $fixture))".repeat(count);
        format!("(component (core module $fixture) {instances})")
    }

    fn push_u32_leb(bytes: &mut Vec<u8>, mut value: usize) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return;
            }
        }
    }

    fn read_u32_leb(bytes: &[u8], cursor: &mut usize) -> usize {
        let mut value = 0usize;
        let mut shift = 0usize;
        loop {
            let byte = bytes[*cursor];
            *cursor += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn plugin_with_extra_core_instances(
        base: &[u8],
        core_module: &[u8],
        instance_count: usize,
    ) -> Vec<u8> {
        const COMPONENT_HEADER_BYTES: usize = 8;
        const CORE_MODULE_SECTION: u8 = 1;
        const CORE_INSTANCE_SECTION: u8 = 2;

        let mut cursor = COMPONENT_HEADER_BYTES;
        let mut module_index = 0usize;
        while cursor < base.len() {
            let section_id = base[cursor];
            cursor += 1;
            let section_len = read_u32_leb(base, &mut cursor);
            if section_id == CORE_MODULE_SECTION {
                module_index += 1;
            }
            cursor += section_len;
        }
        assert_eq!(cursor, base.len());

        let mut bytes = base.to_vec();
        bytes.push(CORE_MODULE_SECTION);
        push_u32_leb(&mut bytes, core_module.len());
        bytes.extend_from_slice(core_module);

        let mut instances = Vec::new();
        push_u32_leb(&mut instances, instance_count);
        for _ in 0..instance_count {
            instances.push(0x00);
            push_u32_leb(&mut instances, module_index);
            instances.push(0x00);
        }
        bytes.push(CORE_INSTANCE_SECTION);
        push_u32_leb(&mut bytes, instances.len());
        bytes.extend(instances);
        bytes
    }

    fn loaded_binary_fixture(runtime: &WasmRuntime, id: &str, bytes: &[u8]) -> LoadedPlugin {
        LoadedPlugin {
            id: id.to_string(),
            component: Component::from_binary(&runtime.engine, bytes).unwrap(),
            engine: runtime.engine.clone(),
            epoch_ticker: runtime.epoch_ticker.clone(),
            _path: std::path::PathBuf::from("<resource-limit-test>"),
        }
    }

    fn assert_unavailable_reason(result: Result<PluginInstance>, expected: &str) {
        match result {
            Err(PluginError::Unavailable(reason)) => assert_eq!(reason, expected),
            Err(error) => panic!("expected redacted unavailable error, got {error}"),
            Ok(_) => panic!("resource-limit fixture unexpectedly instantiated"),
        }
    }

    #[test]
    fn runtime_engine_enables_fuel_metering() {
        let runtime = WasmRuntime::new().unwrap();
        let mut store = Store::new(&runtime.engine, ());

        store
            .set_fuel(1)
            .expect("every runtime store must support fuel metering");
    }

    // A component whose export makes exactly one host call and then returns
    // through a short loop (the loop backedge is where wasmtime inserts an
    // epoch check, so a deadline that expired during the hostcall is
    // observed on return). The hostcall itself does nothing but take
    // wall-clock time -- the shape of a network/disk hostcall, or of a
    // worker thread descheduled on a loaded machine.
    const SLOW_HOSTCALL_COMPONENT_WAT: &str = r#"
        (component
            (import "block" (func $host-block))
            (core module $m
                (import "host" "block" (func $block))
                (func (export "run")
                    (local $i i32)
                    call $block
                    (local.set $i (i32.const 8))
                    (loop $spin
                        (local.set $i (i32.sub (local.get $i) (i32.const 1)))
                        (br_if $spin (i32.ne (local.get $i) (i32.const 0)))
                    )
                )
            )
            (core func $block-lowered (canon lower (func $host-block)))
            (core instance $host-inst
                (export "block" (func $block-lowered))
            )
            (core instance $inst (instantiate $m (with "host" (instance $host-inst))))
            (func (export "run") (canon lift (core func $inst "run")))
        )
    "#;

    // The wedge shape the epoch deadline exists for: an unbounded loop over
    // a cheap hostcall. Each iteration burns a handful of fuel but an
    // arbitrary amount of wall-clock, so fuel alone would let this pin the
    // plugin worker for minutes.
    const WEDGED_HOSTCALL_LOOP_WAT: &str = r#"
        (component
            (import "block" (func $host-block))
            (core module $m
                (import "host" "block" (func $block))
                (func (export "run")
                    (loop $forever
                        call $block
                        br $forever
                    )
                )
            )
            (core func $block-lowered (canon lower (func $host-block)))
            (core instance $host-inst
                (export "block" (func $block-lowered))
            )
            (core instance $inst (instantiate $m (with "host" (instance $host-inst))))
            (func (export "run") (canon lift (core func $inst "run")))
        )
    "#;

    fn instantiate_hostcall_component(
        runtime: &WasmRuntime,
        wat: &str,
        hostcall_duration: Duration,
    ) -> (Store<HostFunctions>, wasmtime::component::TypedFunc<(), ()>) {
        let component = Component::new(&runtime.engine, wat).unwrap();
        let host =
            HostFunctions::new_for_metadata_validation("epoch-deadline-test".to_string()).unwrap();
        let mut store = new_plugin_store(&runtime.engine, host).unwrap();
        let mut linker: Linker<HostFunctions> = Linker::new(&runtime.engine);
        linker
            .root()
            .func_wrap("block", move |_store, (): ()| {
                std::thread::sleep(hostcall_duration);
                Ok(())
            })
            .unwrap();
        let instance = linker.instantiate(&mut store, &component).unwrap();
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();
        (store, run)
    }

    /// The property the epoch deadline broke when it was a ~100ms budget: a
    /// guest whose export spends wall-clock in a single legitimate hostcall
    /// (150ms here -- above the old ten-tick ceiling) while consuming almost
    /// no fuel must complete, twice in a row, with the instance staying
    /// available. Against `EPOCH_TICKS_PER_EXPORT = 10` this fails with
    /// `Unavailable("plugin execution deadline exceeded")` on the first call.
    #[test]
    fn legitimate_slow_hostcall_work_is_not_trapped_by_the_epoch_deadline() {
        let runtime = WasmRuntime::new().unwrap();
        let (mut store, run) = instantiate_hostcall_component(
            &runtime,
            SLOW_HOSTCALL_COMPONENT_WAT,
            Duration::from_millis(150),
        );
        let mut availability = InstanceAvailability::default();

        for call in 0..2 {
            call_with_quotas(
                &mut store,
                &mut availability,
                |store| run.call(store, ()),
                |_| Ok(()),
                PluginError::ExecutionError,
            )
            .unwrap_or_else(|error| {
                panic!("well-behaved slow-hostcall export was rejected on call {call}: {error}")
            });
        }
        let fuel_used = FUEL_PER_EXPORT - store.get_fuel().unwrap();
        assert!(
            fuel_used < 1_000,
            "the slow export must be cheap in fuel (used {fuel_used}) -- wall-clock was its \
             only cost, which is precisely what the deadline must not punish"
        );
        assert_eq!(availability.reason(), None);
    }

    /// The wedge the dead-man switch exists for still dies: an unbounded
    /// hostcall loop burns wall-clock without meaningful fuel, and the
    /// free-running ticker + deadline trap + terminal classification must
    /// reap it. Armed at a test-scale deadline (a few ticks) after
    /// instantiation, because waiting out the real minutes-scale production
    /// ceiling is not a unit test; what this pins is the mechanism the
    /// production constant relies on, and the sizing test below pins the
    /// constant itself.
    #[test]
    fn a_wedged_hostcall_loop_is_still_trapped_by_the_epoch_deadline() {
        let runtime = WasmRuntime::new().unwrap();
        let (mut store, run) = instantiate_hostcall_component(
            &runtime,
            WEDGED_HOSTCALL_LOOP_WAT,
            Duration::from_millis(5),
        );

        store.set_fuel(FUEL_PER_EXPORT).unwrap();
        store.set_epoch_deadline(5);
        let error = run.call(&mut store, ()).unwrap_err();

        assert_eq!(
            resource_quota_reason(&error),
            Some("plugin execution deadline exceeded"),
            "a hostcall loop that outlives the epoch deadline must trap terminally"
        );
        let fuel_used = FUEL_PER_EXPORT - store.get_fuel().unwrap_or(0);
        assert!(
            fuel_used < FUEL_PER_EXPORT / 100,
            "the wedge burned only {fuel_used} fuel -- fuel alone could not have reaped it"
        );
    }

    /// Pins the floor the dead-man switch may never shrink below: four
    /// network-request timeouts, the bottom of the minutes band. The
    /// binding constraint is the doctrine on `EPOCH_TICKS_PER_EXPORT`
    /// itself -- the ceiling must dwarf the slowest legitimate export,
    /// which is a few sequential hostcalls each bounded by the network
    /// layer's 30s per-request contract -- and this assertion is the
    /// arithmetic floor beneath that doctrine, so a re-tune back toward a
    /// per-export "budget" fails a test instead of only contradicting a
    /// comment. The shipped value sits at ten request-timeouts,
    /// comfortably above the floor; anything inside the minutes band
    /// satisfies both.
    #[test]
    fn the_epoch_deadline_dwarfs_the_slowest_legitimate_hostcall() {
        let ceiling = EPOCH_TICK_INTERVAL * u32::try_from(EPOCH_TICKS_PER_EXPORT).unwrap();
        assert!(
            ceiling >= 4 * arclain_network::DEFAULT_REQUEST_TIMEOUT,
            "epoch ceiling {ceiling:?} must stay at or above four times the network \
             layer's per-request timeout ({:?}) -- it is a liveness backstop sized for \
             the slowest legitimate export, not a work budget",
            arclain_network::DEFAULT_REQUEST_TIMEOUT
        );
    }

    // Working-set telemetry is deliberately OS-specific; functional boundary
    // coverage for hostcall fuel remains platform-independent below.
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn hostcall_fuel_prevents_large_prevalidation_lift_allocation() {
        if std::env::var_os(HOSTCALL_MEMORY_CHILD).is_some() {
            let runtime = WasmRuntime::new().unwrap();
            let (mut store, lift) =
                instantiate_byte_list_fixture(&runtime, LARGE_LIFT_BYTES).unwrap();
            let before = current_working_set_bytes();
            let lifted = lift.call(&mut store, ()).ok();
            std::hint::black_box(&lifted);
            let delta = current_working_set_bytes().saturating_sub(before);
            println!(
                "ARCLAIN_HOSTCALL_MEASUREMENT lifted={} rss_delta={delta}",
                lifted.is_some()
            );
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::resource_limit_tests::hostcall_fuel_prevents_large_prevalidation_lift_allocation",
                "--nocapture",
            ])
            .env(HOSTCALL_MEMORY_CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let measurement = stdout
            .lines()
            .find(|line| line.contains("ARCLAIN_HOSTCALL_MEASUREMENT"))
            .unwrap_or_else(|| panic!("child did not report memory measurement:\n{stdout}"));
        let lifted = measurement.contains("lifted=true");
        let delta = measurement
            .split("rss_delta=")
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap();

        assert!(
            !lifted && delta < 32 * 1024 * 1024,
            "a 120 MiB guest result was lifted before validation: {measurement}"
        );
    }

    #[test]
    fn configured_hostcall_fuel_covers_measured_current_wit_shapes() {
        use wirt::bindings::wirt::plugin::rules::PluginRuleDefinition as WitRule;
        use wirt::bindings::wirt::plugin::ui::{
            KeyValuePair as WitKeyValuePair, ListItemConfig as WitListItem,
            PluginAction as WitAction, ToolbarButtonConfig as WitToolbarButton,
            TopTabConfig as WitTopTab, UiElement as WitUiElement,
        };

        const DOCUMENTED_HOSTCALL_FUEL_BYTES: usize = 8 * 1024 * 1024;
        assert!(
            crate::types::MAX_PLUGIN_GUEST_DATA_BYTES <= HOSTCALL_FUEL_BYTES / 2,
            "guest-return body cap must leave at least half of actual hostcall fuel for WIT lifting overhead"
        );

        fn max_serialized_list_len<T: Serialize>(minimum_item: &T) -> usize {
            let item_bytes = serde_json::to_vec(minimum_item).unwrap().len();
            (MAX_SERIALIZED_RESULT_BYTES - 2) / (item_bytes + 1) + 1
        }

        let largest_ui_allocation = [
            std::mem::size_of::<WitUiElement>(),
            std::mem::size_of::<WitListItem>(),
            std::mem::size_of::<WitToolbarButton>(),
            std::mem::size_of::<WitKeyValuePair>(),
            std::mem::size_of::<(String, Option<String>)>(),
            std::mem::size_of::<String>(),
        ]
        .into_iter()
        .max()
        .unwrap();
        let layout_bytes =
            MAX_SERIALIZED_RESULT_BYTES + MAX_UI_ELEMENTS.saturating_mul(largest_ui_allocation);

        let action_bytes = MAX_SERIALIZED_RESULT_BYTES
            + MAX_ACTIONS.saturating_mul(
                std::mem::size_of::<WitAction>() + std::mem::size_of::<(String, Option<String>)>(),
            );

        let minimum_tab = crate::types::TopTabConfig {
            id: String::new(),
            label: String::new(),
            icon: String::new(),
            badge: None,
            priority: 0,
        };
        let top_tab_bytes = MAX_SERIALIZED_RESULT_BYTES
            + max_serialized_list_len(&minimum_tab)
                .saturating_mul(std::mem::size_of::<WitTopTab>());

        let minimum_rule = arclain_core::OrganizationRule::default();
        let rule_bytes = MAX_SERIALIZED_RESULT_BYTES
            + max_serialized_list_len(&minimum_rule).saturating_mul(std::mem::size_of::<WitRule>());

        let worst_shape_bytes = [layout_bytes, action_bytes, top_tab_bytes, rule_bytes]
            .into_iter()
            .max()
            .unwrap();
        assert!(
            worst_shape_bytes <= DOCUMENTED_HOSTCALL_FUEL_BYTES,
            "current WIT host shape needs {worst_shape_bytes} bytes of hostcall fuel"
        );

        let runtime = WasmRuntime::new().unwrap();
        let host =
            HostFunctions::new_for_metadata_validation("hostcall-budget".to_string()).unwrap();
        let store = new_plugin_store(&runtime.engine, host).unwrap();
        assert_eq!(
            store.hostcall_fuel(),
            DOCUMENTED_HOSTCALL_FUEL_BYTES,
            "production stores must use the measured current-WIT hostcall budget; estimates: layout={layout_bytes}, actions={action_bytes}, tabs={top_tab_bytes}, rules={rule_bytes}"
        );
    }

    #[test]
    fn hostcall_fuel_accepts_exact_boundary_and_terminally_rejects_one_over() {
        let runtime = WasmRuntime::new().unwrap();
        let (mut exact_store, exact_lift) =
            instantiate_byte_list_fixture(&runtime, HOSTCALL_FUEL_BYTES).unwrap();
        let exact = exact_lift.call(&mut exact_store, ()).unwrap().0;
        assert_eq!(exact.len(), HOSTCALL_FUEL_BYTES);

        let (mut over_store, over_lift) =
            instantiate_byte_list_fixture(&runtime, HOSTCALL_FUEL_BYTES + 1).unwrap();
        let mut availability = InstanceAvailability::default();
        let call_entries = std::sync::atomic::AtomicUsize::new(0);
        let first = call_with_quotas(
            &mut over_store,
            &mut availability,
            |store| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                over_lift.call(store, ()).map(|_| ())
            },
            |_| Ok(()),
            PluginError::ExecutionError,
        )
        .unwrap_err();
        let second = call_with_quotas(
            &mut over_store,
            &mut availability,
            |store| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                over_lift.call(store, ()).map(|_| ())
            },
            |_| Ok(()),
            PluginError::ExecutionError,
        )
        .unwrap_err();

        assert!(matches!(
            first,
            PluginError::Unavailable(ref reason)
                if reason == "plugin hostcall data quota exceeded"
        ));
        assert!(matches!(second, PluginError::Unavailable(_)));
        assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            availability.reason(),
            Some("plugin hostcall data quota exceeded")
        );
    }

    #[test]
    fn fuel_epoch_memory_and_table_quota_errors_have_redacted_classifications() {
        let fuel = wasmtime::Error::from(wasmtime::Trap::OutOfFuel);
        let epoch = wasmtime::Error::from(wasmtime::Trap::Interrupt);
        let memory = wasmtime::Error::new(crate::host_functions::StoreQuotaExceeded {
            kind: crate::host_functions::StoreQuotaKind::Memory,
        });
        let table = wasmtime::Error::new(crate::host_functions::StoreQuotaExceeded {
            kind: crate::host_functions::StoreQuotaKind::Table,
        });

        assert_eq!(
            resource_quota_reason(&fuel),
            Some("plugin fuel quota exceeded")
        );
        assert_eq!(
            resource_quota_reason(&epoch),
            Some("plugin execution deadline exceeded")
        );
        assert_eq!(
            resource_quota_reason(&memory),
            Some("plugin memory quota exceeded")
        );
        assert_eq!(
            resource_quota_reason(&table),
            Some("plugin table quota exceeded")
        );
    }

    /// A guest trap that is *not* one of the two quota-shaped variants
    /// (a real out-of-bounds panic, `unreachable`, an integer division
    /// by zero, ...) must still be classified terminal -- see
    /// `resource_quota_reason`'s own doc comment for why every trap
    /// variant permanently poisons its `Store`, not just fuel/interrupt.
    #[test]
    fn a_generic_guest_trap_is_also_classified_as_terminal() {
        let unreachable = wasmtime::Error::from(wasmtime::Trap::UnreachableCodeReached);
        let division_by_zero = wasmtime::Error::from(wasmtime::Trap::IntegerDivisionByZero);

        assert_eq!(
            resource_quota_reason(&unreachable),
            Some("plugin execution trapped")
        );
        assert_eq!(
            resource_quota_reason(&division_by_zero),
            Some("plugin execution trapped")
        );
    }

    #[test]
    fn serialized_result_limit_accepts_exact_boundary_and_rejects_one_byte_over() {
        let exact = "x".repeat(MAX_SERIALIZED_RESULT_BYTES - 2);
        let over = "x".repeat(MAX_SERIALIZED_RESULT_BYTES - 1);

        assert!(validate_serialized_result(&exact).is_ok());
        assert!(matches!(
            validate_serialized_result(&over),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    #[test]
    fn ui_element_limit_counts_nested_elements_at_the_boundary() {
        use crate::types::{PluginLayout, PluginUiElement};

        let nested_items = vec![PluginUiElement::Separator; MAX_UI_ELEMENTS - 1];
        let exact = PluginLayout::Single {
            elements: vec![PluginUiElement::ListContainer {
                id: "items".to_string(),
                items: nested_items,
                max_height: None,
                empty_message: None,
            }],
        };
        assert!(validate_layout_result(&exact).is_ok());

        let over = PluginLayout::Single {
            elements: vec![PluginUiElement::Separator; MAX_UI_ELEMENTS + 1],
        };
        assert!(matches!(
            validate_layout_result(&over),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    #[test]
    fn every_rendered_layout_collection_counts_boundary_and_one_over() {
        use crate::types::{KeyValuePair, PluginLayout, PluginUiElement, ToolbarButton};

        let strings = |count| vec![String::new(); count];
        let toolbar_buttons = |count| {
            vec![
                ToolbarButton {
                    id: String::new(),
                    label: String::new(),
                    icon: None,
                    primary: false,
                    spacer_before: false,
                };
                count
            ]
        };
        let pairs = |count| {
            vec![
                KeyValuePair {
                    key: String::new(),
                    value: String::new(),
                };
                count
            ]
        };
        let images = |count| vec![(String::new(), None); count];
        let single = |element| PluginLayout::Single {
            elements: vec![element],
        };

        let cases = vec![
            (
                "radio options",
                single(PluginUiElement::RadioGroup {
                    id: String::new(),
                    label: String::new(),
                    options: strings(MAX_UI_ELEMENTS - 1),
                    selected: String::new(),
                }),
                single(PluginUiElement::RadioGroup {
                    id: String::new(),
                    label: String::new(),
                    options: strings(MAX_UI_ELEMENTS),
                    selected: String::new(),
                }),
            ),
            (
                "dropdown options",
                single(PluginUiElement::Dropdown {
                    id: String::new(),
                    label: String::new(),
                    options: strings(MAX_UI_ELEMENTS - 1),
                    selected: String::new(),
                }),
                single(PluginUiElement::Dropdown {
                    id: String::new(),
                    label: String::new(),
                    options: strings(MAX_UI_ELEMENTS),
                    selected: String::new(),
                }),
            ),
            (
                "tabs",
                single(PluginUiElement::Tabs {
                    id: String::new(),
                    tabs: strings(MAX_UI_ELEMENTS - 1),
                    selected: String::new(),
                }),
                single(PluginUiElement::Tabs {
                    id: String::new(),
                    tabs: strings(MAX_UI_ELEMENTS),
                    selected: String::new(),
                }),
            ),
            (
                "toolbar buttons",
                single(PluginUiElement::Toolbar {
                    buttons: toolbar_buttons(MAX_UI_ELEMENTS - 1),
                }),
                single(PluginUiElement::Toolbar {
                    buttons: toolbar_buttons(MAX_UI_ELEMENTS),
                }),
            ),
            (
                "carousel images",
                single(PluginUiElement::Carousel {
                    id: String::new(),
                    images: images(MAX_UI_ELEMENTS - 1),
                    current_index: 0,
                    max_height: None,
                    thumbnail_height: None,
                    enable_lightbox: true,
                }),
                single(PluginUiElement::Carousel {
                    id: String::new(),
                    images: images(MAX_UI_ELEMENTS),
                    current_index: 0,
                    max_height: None,
                    thumbnail_height: None,
                    enable_lightbox: true,
                }),
            ),
            (
                "key-value list items",
                single(PluginUiElement::KeyValueList {
                    items: pairs(MAX_UI_ELEMENTS - 1),
                    columns: None,
                }),
                single(PluginUiElement::KeyValueList {
                    items: pairs(MAX_UI_ELEMENTS),
                    columns: None,
                }),
            ),
            (
                "metadata grid items",
                single(PluginUiElement::MetadataGrid {
                    items: pairs(MAX_UI_ELEMENTS - 1),
                    columns: None,
                }),
                single(PluginUiElement::MetadataGrid {
                    items: pairs(MAX_UI_ELEMENTS),
                    columns: None,
                }),
            ),
            (
                "tag chips",
                single(PluginUiElement::TagChips {
                    tags: strings(MAX_UI_ELEMENTS - 1),
                    max_display: None,
                }),
                single(PluginUiElement::TagChips {
                    tags: strings(MAX_UI_ELEMENTS),
                    max_display: None,
                }),
            ),
        ];

        for (name, exact, over) in cases {
            assert!(
                validate_layout_result(&exact).is_ok(),
                "{name} must accept the exact rendered-work boundary"
            );
            assert!(
                matches!(
                    validate_layout_result(&over),
                    Err(ResultValidationError::Quota(QuotaViolation::Result))
                ),
                "{name} must reject one rendered work item over"
            );
        }
    }

    #[test]
    fn tag_chip_work_matches_visible_tags_plus_overflow_label() {
        let layout = crate::types::PluginLayout::Single {
            elements: vec![crate::types::PluginUiElement::TagChips {
                tags: vec![String::new(); MAX_UI_ELEMENTS + 1],
                max_display: Some(0),
            }],
        };

        assert!(
            validate_layout_result(&layout).is_ok(),
            "zero visible tags render one element plus the +N-more label"
        );
    }

    #[test]
    fn split_nested_layout_combines_all_rendered_work() {
        use crate::types::{PluginLayout, PluginUiElement, ToolbarButton};

        let layout = |button_count| PluginLayout::Split {
            sidebar: vec![PluginUiElement::ListContainer {
                id: String::new(),
                items: vec![PluginUiElement::Separator; 4_998],
                max_height: None,
                empty_message: None,
            }],
            content: vec![PluginUiElement::Toolbar {
                buttons: vec![
                    ToolbarButton {
                        id: String::new(),
                        label: String::new(),
                        icon: None,
                        primary: false,
                        spacer_before: false,
                    };
                    button_count
                ],
            }],
            sidebar_width: None,
        };

        assert!(validate_layout_result(&layout(5_000)).is_ok());
        assert!(matches!(
            validate_layout_result(&layout(5_001)),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    #[test]
    fn action_limit_accepts_boundary_and_rejects_one_over() {
        let exact = vec![crate::types::PluginAction::None; MAX_ACTIONS];
        let over = vec![crate::types::PluginAction::None; MAX_ACTIONS + 1];

        assert!(validate_actions_result(&exact).is_ok());
        assert!(matches!(
            validate_actions_result(&over),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    #[test]
    fn lightbox_images_share_the_action_work_budget() {
        let action = |image_count| {
            vec![crate::types::PluginAction::OpenLightbox {
                images: vec![(String::new(), None); image_count],
                start_index: 0,
                title: None,
            }]
        };

        assert!(validate_actions_result(&action(MAX_ACTIONS - 1)).is_ok());
        assert!(matches!(
            validate_actions_result(&action(MAX_ACTIONS)),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    fn minimal_top_tab() -> crate::types::TopTabConfig {
        crate::types::TopTabConfig {
            id: String::new(),
            label: String::new(),
            icon: String::new(),
            badge: None,
            priority: 0,
        }
    }

    #[test]
    fn top_tab_limit_accepts_boundary_and_rejects_one_over() {
        let exact = vec![minimal_top_tab(); MAX_UI_ELEMENTS];
        let over = vec![minimal_top_tab(); MAX_UI_ELEMENTS + 1];

        assert!(validate_top_tabs_result(&exact).is_ok());
        assert!(matches!(
            validate_top_tabs_result(&over),
            Err(ResultValidationError::Quota(QuotaViolation::Result))
        ));
    }

    #[test]
    fn oversized_top_tabs_are_terminal_and_second_call_skips_guest_boundary() {
        let runtime = WasmRuntime::new().unwrap();
        let host = HostFunctions::new_for_metadata_validation("top-tab-quota".to_string()).unwrap();
        let mut store = new_plugin_store(&runtime.engine, host).unwrap();
        let mut availability = InstanceAvailability::default();
        let call_entries = std::sync::atomic::AtomicUsize::new(0);
        let over = vec![minimal_top_tab(); MAX_UI_ELEMENTS + 1];

        let first = call_with_quotas(
            &mut store,
            &mut availability,
            |_| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(over.clone())
            },
            |tabs| validate_top_tabs_result(tabs),
            PluginError::ExecutionError,
        )
        .unwrap_err();
        let second = call_with_quotas(
            &mut store,
            &mut availability,
            |_| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<crate::types::TopTabConfig>::new())
            },
            |tabs| validate_top_tabs_result(tabs),
            PluginError::ExecutionError,
        )
        .unwrap_err();

        assert!(matches!(
            first,
            PluginError::Unavailable(ref reason) if reason == "plugin result quota exceeded"
        ));
        assert!(matches!(second, PluginError::Unavailable(_)));
        assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(availability.reason(), Some("plugin result quota exceeded"));
    }

    #[test]
    fn linear_memory_limit_accepts_boundary_and_rejects_one_page_over() {
        let runtime = WasmRuntime::new().unwrap();

        instantiate_resource_fixture(&runtime, "(memory 4096)")
            .expect("256 MiB linear memory must remain available");
        let over_limit = instantiate_resource_fixture(&runtime, "(memory 4097)");
        assert!(
            over_limit.is_err(),
            "linear memory above 256 MiB must be rejected"
        );
    }

    #[test]
    fn table_element_limit_accepts_boundary_and_rejects_one_over() {
        let runtime = WasmRuntime::new().unwrap();

        instantiate_resource_fixture(&runtime, "(table 100000 funcref)")
            .expect("100,000 table elements must remain available");
        assert!(
            instantiate_resource_fixture(&runtime, "(table 100001 funcref)").is_err(),
            "table element counts above 100,000 must be rejected"
        );
    }

    #[test]
    fn table_count_limit_accepts_eight_and_rejects_nine() {
        let runtime = WasmRuntime::new().unwrap();
        let at_limit = "(table 1 funcref)".repeat(8);
        let over_limit = "(table 1 funcref)".repeat(9);

        instantiate_resource_fixture(&runtime, &at_limit)
            .expect("eight tables must remain available");
        assert!(
            instantiate_resource_fixture(&runtime, &over_limit).is_err(),
            "a ninth table must be rejected"
        );
    }

    #[test]
    fn memory_count_limit_accepts_four_and_rejects_five() {
        let runtime = WasmRuntime::new().unwrap();
        let at_limit = "(memory 1)".repeat(4);
        let over_limit = "(memory 1)".repeat(5);

        instantiate_resource_fixture(&runtime, &at_limit)
            .expect("four memories must remain available");
        assert!(
            instantiate_resource_fixture(&runtime, &over_limit).is_err(),
            "a fifth memory must be rejected"
        );
    }

    #[test]
    fn core_instance_limit_accepts_compatibility_boundary_and_rejects_one_over() {
        let runtime = WasmRuntime::new().unwrap();
        instantiate_core_instance_fixture(&runtime, crate::host_functions::MAX_CORE_INSTANCES)
            .expect("the compatibility-safe core-instance boundary must remain available");
        assert!(
            instantiate_core_instance_fixture(
                &runtime,
                crate::host_functions::MAX_CORE_INSTANCES + 1
            )
            .is_err(),
            "a core instance above the compatibility boundary must be rejected"
        );
    }

    #[test]
    fn locked_wasmtime_resource_count_causes_are_redacted_quota_errors() {
        let runtime = WasmRuntime::new().unwrap();
        let instance = instantiate_core_instance_fixture(
            &runtime,
            crate::host_functions::MAX_CORE_INSTANCES + 1,
        )
        .unwrap_err();
        let memory = instantiate_resource_fixture(&runtime, &"(memory 1)".repeat(5)).unwrap_err();
        let table =
            instantiate_resource_fixture(&runtime, &"(table 1 funcref)".repeat(9)).unwrap_err();

        assert_eq!(
            instance.root_cause().to_string(),
            "resource limit exceeded: instance count too high at 33"
        );
        assert_eq!(
            memory.root_cause().to_string(),
            "resource limit exceeded: memory count too high at 5"
        );
        assert_eq!(
            table.root_cause().to_string(),
            "resource limit exceeded: table count too high at 9"
        );
        assert_eq!(
            resource_quota_reason(&instance),
            Some("plugin instance quota exceeded")
        );
        assert_eq!(
            resource_quota_reason(&memory),
            Some("plugin memory quota exceeded")
        );
        assert_eq!(
            resource_quota_reason(&table),
            Some("plugin table quota exceeded")
        );
    }

    #[test]
    fn resource_count_errors_are_redacted_in_both_plugin_instantiation_modes() {
        let runtime = WasmRuntime::new().unwrap();
        let base = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/ui-demo/ui-demo.wasm"
        ));
        const EMPTY_CORE_MODULE: &[u8] = b"\0asm\x01\0\0\0";
        // ui-demo already owns one linear memory and two tables. These modules
        // take the real plugin to Wasmtime's first rejected totals (5 and 9).
        const FOUR_MEMORY_CORE_MODULE: &[u8] =
            b"\0asm\x01\0\0\0\x05\x09\x04\0\x01\0\x01\0\x01\0\x01";
        const SEVEN_TABLE_CORE_MODULE: &[u8] = b"\0asm\x01\0\0\0\x04\x16\x07\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01\x70\0\x01";
        let fixtures = [
            (
                "instance-count",
                plugin_with_extra_core_instances(
                    base,
                    EMPTY_CORE_MODULE,
                    crate::host_functions::MAX_CORE_INSTANCES + 1,
                ),
                "plugin instance quota exceeded",
            ),
            (
                "memory-count",
                plugin_with_extra_core_instances(base, FOUR_MEMORY_CORE_MODULE, 1),
                "plugin memory quota exceeded",
            ),
            (
                "table-count",
                plugin_with_extra_core_instances(base, SEVEN_TABLE_CORE_MODULE, 1),
                "plugin table quota exceeded",
            ),
        ];
        let plugin_log_dir = tempfile::tempdir().unwrap();

        for (id, bytes, expected) in fixtures {
            let loaded = loaded_binary_fixture(&runtime, id, &bytes);
            assert_unavailable_reason(
                loaded.instantiate_with_plugin_log_dir(
                    vec![],
                    10,
                    HashMap::new(),
                    None,
                    plugin_log_dir.path(),
                ),
                expected,
            );
            assert_unavailable_reason(loaded.instantiate_for_metadata_validation(), expected);
        }
    }

    #[test]
    fn bundled_components_instantiate_with_the_compatibility_safe_core_instance_limit() {
        let runtime = WasmRuntime::new().unwrap();
        let plugin_log_dir = tempfile::tempdir().unwrap();
        for (id, bytes) in [
            (
                "ui-demo",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../plugins/ui-demo/ui-demo.wasm"
                ))
                .as_slice(),
            ),
            (
                "malicious-metadata",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/malicious-metadata/malicious-metadata.wasm"
                ))
                .as_slice(),
            ),
        ] {
            let loaded = runtime
                .load_module_from_bytes(id.to_string(), bytes)
                .unwrap();
            loaded
                .instantiate_with_plugin_log_dir(
                    vec![],
                    10,
                    HashMap::new(),
                    None,
                    plugin_log_dir.path(),
                )
                .unwrap_or_else(|error| {
                    panic!("{id} must instantiate normally under all quotas: {error}")
                });
            let mut instance = loaded
                .instantiate_for_metadata_validation()
                .unwrap_or_else(|error| panic!("{id} must fit the core-instance quota: {error}"));
            instance
                .get_metadata()
                .unwrap_or_else(|error| panic!("{id} metadata must execute under quotas: {error}"));
        }
    }

    #[test]
    fn runtime_drop_stops_and_joins_epoch_ticker() {
        let runtime = WasmRuntime::new().unwrap();
        let ticker_exited = runtime.epoch_ticker_exit_probe();

        drop(runtime);

        assert!(
            ticker_exited.load(std::sync::atomic::Ordering::Acquire),
            "runtime Drop must join the epoch ticker before returning"
        );
    }

    #[test]
    fn infinite_component_becomes_terminal_and_second_call_never_reenters_guest() {
        let (outcome_tx, outcome_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let outcome = (|| -> anyhow::Result<(PluginError, PluginError, i32, Duration)> {
                let runtime = WasmRuntime::new()?;
                let component = Component::new(
                    &runtime.engine,
                    r#"
                        (component
                            (core module $fixture
                                (func (export "run")
                                    (loop $spin (br $spin))))
                            (core instance $instance (instantiate $fixture))
                            (func (export "run")
                                (canon lift (core func $instance "run"))))
                    "#,
                )?;
                let host = HostFunctions::new_for_metadata_validation("terminal-test".to_string())?;
                let mut store = new_plugin_store(&runtime.engine, host)?;
                let instance = Linker::new(&runtime.engine).instantiate(&mut store, &component)?;
                let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
                let mut availability = InstanceAvailability::default();
                let call_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));

                let first_entries = call_entries.clone();
                let first = call_with_quotas(
                    &mut store,
                    &mut availability,
                    |store| {
                        first_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        run.call(store, ())
                    },
                    |_| Ok(()),
                    PluginError::ExecutionError,
                )
                .unwrap_err();

                let second_started = std::time::Instant::now();
                let second_entries = call_entries.clone();
                let second = call_with_quotas(
                    &mut store,
                    &mut availability,
                    |store| {
                        second_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        run.call(store, ())
                    },
                    |_| Ok(()),
                    PluginError::ExecutionError,
                )
                .unwrap_err();
                let second_elapsed = second_started.elapsed();
                let guest_entries = call_entries.load(std::sync::atomic::Ordering::SeqCst) as i32;
                Ok((first, second, guest_entries, second_elapsed))
            })();
            let _ = outcome_tx.send(outcome);
        });

        let (first, second, guest_entries, second_elapsed) = outcome_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("infinite guest call must terminate under a wall-clock guard")
            .unwrap();
        assert!(matches!(first, PluginError::Unavailable(_)));
        assert!(matches!(second, PluginError::Unavailable(_)));
        assert_eq!(
            guest_entries, 1,
            "the terminal call must not enter the guest"
        );
        assert!(
            second_elapsed < Duration::from_millis(10),
            "terminal calls must fail before crossing the Wasm boundary"
        );
    }

    #[test]
    fn ordinary_guest_error_does_not_make_instance_terminal() {
        let runtime = WasmRuntime::new().unwrap();
        let host =
            HostFunctions::new_for_metadata_validation("ordinary-error".to_string()).unwrap();
        let mut store = new_plugin_store(&runtime.engine, host).unwrap();
        let mut availability = InstanceAvailability::default();
        let call_entries = std::sync::atomic::AtomicUsize::new(0);

        let first = call_with_quotas(
            &mut store,
            &mut availability,
            |_| -> wasmtime::Result<()> {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(wasmtime::Error::msg("guest-owned secret"))
            },
            |_| Ok(()),
            PluginError::ExecutionError,
        )
        .unwrap_err();
        let second = call_with_quotas(
            &mut store,
            &mut availability,
            |_| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            |_| Ok(()),
            PluginError::ExecutionError,
        );

        assert!(
            matches!(first, PluginError::ExecutionError(message) if message == "guest-owned secret")
        );
        assert!(second.is_ok());
        assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(availability.reason(), None);
    }

    #[test]
    fn oversized_result_is_terminal_redacted_and_second_call_skips_guest_boundary() {
        let runtime = WasmRuntime::new().unwrap();
        let host = HostFunctions::new_for_metadata_validation("result-quota".to_string()).unwrap();
        let mut store = new_plugin_store(&runtime.engine, host).unwrap();
        let mut availability = InstanceAvailability::default();
        let call_entries = std::sync::atomic::AtomicUsize::new(0);
        let secret = "guest-secret";

        let first = call_with_quotas(
            &mut store,
            &mut availability,
            |_| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(format!(
                    "{secret}{}",
                    "x".repeat(MAX_SERIALIZED_RESULT_BYTES)
                ))
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )
        .unwrap_err();
        let second = call_with_quotas(
            &mut store,
            &mut availability,
            |_| {
                call_entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(String::new())
            },
            validate_serialized_result,
            PluginError::ExecutionError,
        )
        .unwrap_err();

        assert!(
            matches!(first, PluginError::Unavailable(ref reason) if reason == "plugin result quota exceeded" && !reason.contains(secret))
        );
        assert!(matches!(second, PluginError::Unavailable(_)));
        assert_eq!(call_entries.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(availability.reason(), Some("plugin result quota exceeded"));
    }
}
